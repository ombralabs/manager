//! Процессы автоматизации (спецификация автоматизации, «Хранилище»): JSON процесса как в файле экспорта, без
//! `subflows`; имя и группа дублируются в столбцы для списка.
use super::{Store, StoreError};
use crate::flow::model::Flow;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, PartialEq)]
pub struct FlowRecord {
    pub id: i64,
    pub quota_id: String,
    pub flow: Flow,
    pub created_at: i64,
    pub updated_at: i64,
}

fn resource_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random for RPA identity");
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

impl Store {
    pub fn create_flow(&self, flow: &Flow, now: i64) -> Result<FlowRecord, StoreError> {
        let body = body(flow)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO flows (name, grp, body, created_at, updated_at, quota_id) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![flow.name, flow.group, body, now, resource_id()],
        )?;
        let id = conn.last_insert_rowid();
        fetch(&conn, id)?.ok_or_else(|| not_found(id))
    }

    pub fn update_flow(&self, id: i64, flow: &Flow, now: i64) -> Result<FlowRecord, StoreError> {
        let body = body(flow)?;
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE flows SET name = ?2, grp = ?3, body = ?4, updated_at = ?5 WHERE id = ?1",
            params![id, flow.name, flow.group, body, now],
        )?;
        if changed == 0 {
            return Err(not_found(id));
        }
        fetch(&conn, id)?.ok_or_else(|| not_found(id))
    }

    pub fn get_flow(&self, id: i64) -> Result<Option<FlowRecord>, StoreError> {
        fetch(&self.conn(), id)
    }

    pub fn delete_flow(&self, id: i64) -> Result<(), StoreError> {
        if self.conn().execute("DELETE FROM flows WHERE id = ?1", [id])? == 0 { return Err(not_found(id)); }
        Ok(())
    }

    /// Все процессы по id. Тело, которое не разбирается, — ошибка списка, как у `get_flow`.
    // ponytail: один битый процесс (ручная правка базы) роняет весь список; пропуск с пометкой, если это проявится.
    pub fn list_flows(&self) -> Result<Vec<FlowRecord>, StoreError> {
        let conn = self.conn();
        let ids: Vec<i64> = {
            let mut statement = conn.prepare("SELECT id FROM flows ORDER BY id")?;
            statement.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        ids.into_iter().map(|id| fetch(&conn, id)?.ok_or_else(|| not_found(id))).collect()
    }

    /// id процессов с таким именем (точное совпадение) — для `ombra flow run <имя>`.
    pub fn flows_named(&self, name: &str) -> Result<Vec<i64>, StoreError> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT id FROM flows WHERE name = ?1 ORDER BY id")?;
        let ids = statement.query_map([name], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(ids)
    }
}

/// Тело, которое не разбирается (ручная правка, база другой версии), — `Invalid`, а не паника.
fn fetch(conn: &Connection, id: i64) -> Result<Option<FlowRecord>, StoreError> {
    let row: Option<(String, i64, i64, String)> = conn
        .query_row("SELECT body, created_at, updated_at, quota_id FROM flows WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()?;
    let Some((body, created_at, updated_at, quota_id)) = row else { return Ok(None) };
    let flow = serde_json::from_str(&body).map_err(|_| StoreError::Invalid("flows.body"))?;
    Ok(Some(FlowRecord { id, quota_id, flow, created_at, updated_at }))
}

/// В базе подпроцессы не лежат: `subflows` — только у экспорта.
fn body(flow: &Flow) -> Result<String, StoreError> {
    let mut flow = flow.clone();
    flow.subflows.clear();
    serde_json::to_string(&flow).map_err(|_| StoreError::Invalid("flow"))
}

fn not_found(id: i64) -> StoreError {
    StoreError::NotFound(format!("сценарий {id}"))
}


impl Store {
    /// Root, imported subflows and remapped links commit together.
    pub fn import_flow_bundle(&self, flow: &Flow, now: i64) -> Result<FlowRecord, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let insert = |flow: &Flow| -> Result<i64, StoreError> {
            tx.execute("INSERT INTO flows(name, grp, body, created_at, updated_at, quota_id) VALUES (?1, ?2, ?3, ?4, ?4, ?5)", params![flow.name, flow.group, body(flow)?, now, resource_id()])?;
            Ok(tx.last_insert_rowid())
        };
        let mut mapping = std::collections::HashMap::new();
        let mut children = Vec::new();
        for (key, sub) in &flow.subflows {
            let new = insert(sub)?;
            if let Ok(old) = key.parse::<i64>() { mapping.insert(old, new); }
            children.push((new, sub));
        }
        for (id, sub) in children {
            let mut sub = sub.clone();
            crate::flow::bundle::remap(&mut sub.nodes, &mapping);
            tx.execute("UPDATE flows SET body = ?1 WHERE id = ?2", params![body(&sub)?, id])?;
        }
        let mut root = flow.clone();
        crate::flow::bundle::remap(&mut root.nodes, &mapping);
        let id = insert(&root)?;
        let record = fetch(&tx, id)?.ok_or_else(|| not_found(id))?;
        tx.commit()?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::model::parse;
    use crate::store::tests::memory_store;

    fn sample(name: &str) -> Flow {
        parse(&format!(
            r#"{{"format":"ombra-flow/1","name":"{name}","group":"Магазины","nodes":[{{"id":"s","op":"start"}}],
                "subflows":{{"9":{{"format":"ombra-flow/1","name":"sub","nodes":[]}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn flows_round_trip_without_subflows() {
        let store = memory_store();
        let created = store.create_flow(&sample("Вход"), 10).unwrap();
        assert_eq!((created.created_at, created.updated_at), (10, 10));
        assert!(created.flow.subflows.is_empty(), "подпроцессы в базе не лежат — только в экспорте");
        let mut expected = sample("Вход");
        expected.subflows.clear();
        assert_eq!(store.get_flow(created.id).unwrap().unwrap().flow, expected);
        let grp: String = store.raw().query_row("SELECT grp FROM flows WHERE id = ?1", [created.id], |r| r.get(0)).unwrap();
        assert_eq!(grp, "Магазины");
    }

    #[test]
    fn update_and_delete() {
        let store = memory_store();
        let id = store.create_flow(&sample("A"), 1).unwrap().id;
        let updated = store.update_flow(id, &sample("B"), 5).unwrap();
        assert_eq!((updated.flow.name.as_str(), updated.created_at, updated.updated_at), ("B", 1, 5));
        store.delete_flow(id).unwrap();
        assert!(store.get_flow(id).unwrap().is_none());
    }

    #[test]
    fn unknown_flow_is_not_found() {
        let store = memory_store();
        assert!(store.get_flow(99).unwrap().is_none());
        assert!(matches!(store.update_flow(99, &sample("A"), 1), Err(StoreError::NotFound(_))));
        assert!(matches!(store.delete_flow(99), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn unreadable_body_is_an_error_not_a_panic() {
        let store = memory_store();
        let id = store.create_flow(&sample("A"), 1).unwrap().id;
        store.raw().execute("UPDATE flows SET body = '{\"format\":' WHERE id = ?1", [id]).unwrap();
        assert!(matches!(store.get_flow(id), Err(StoreError::Invalid("flows.body"))));
    }

    #[test]
    fn flows_are_listed_and_found_by_name() {
        let store = memory_store();
        let a = store.create_flow(&sample("Вход"), 1).unwrap().id;
        let b = store.create_flow(&sample("Поиск"), 2).unwrap().id;
        let c = store.create_flow(&sample("Вход"), 3).unwrap().id;
        assert_eq!(store.list_flows().unwrap().iter().map(|record| record.id).collect::<Vec<_>>(), [a, b, c]);
        assert_eq!(store.flows_named("Вход").unwrap(), [a, c]);
        assert!(store.flows_named("вход").unwrap().is_empty(), "имя — точное совпадение");
    }
}
