//! Экспорт и импорт процесса вместе с подпроцессами (спецификация автоматизации, «Подпроцессы и экспорт»).
use crate::flow::model::{Flow, Node};
use crate::store::{Store, StoreError};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// id процессов, которые вызывает процесс (узлы `control.call`, включая группы), по порядку, без повторов.
pub fn called(flow: &Flow) -> Vec<i64> {
    let mut ids = Vec::new();
    collect(&flow.nodes, &mut ids);
    ids
}

fn collect(nodes: &[Node], ids: &mut Vec<i64>) {
    for node in nodes {
        if node.op == "control.call"
            && let Some(id) = node.config.get("process").and_then(Value::as_i64)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
        collect(&node.nodes, ids);
    }
}

/// Процесс из базы со всеми вызываемыми (рекурсивно, каждый один раз) в `subflows`. Пропавший подпроцесс не
/// кладётся — проверка процесса назовёт его.
pub fn export(store: &Store, id: i64) -> Result<Flow, StoreError> {
    let mut root = store.get_flow(id)?.ok_or_else(|| StoreError::NotFound(format!("сценарий {id}")))?.flow;
    let mut subflows = BTreeMap::new();
    let mut queue = called(&root);
    while let Some(next) = queue.pop() {
        if next == id || subflows.contains_key(&next.to_string()) {
            continue;
        }
        let Some(record) = store.get_flow(next)? else { continue };
        queue.extend(called(&record.flow));
        subflows.insert(next.to_string(), record.flow);
    }
    root.subflows = subflows;
    Ok(root)
}

/// Новые записи для процесса и его `subflows`; ссылки `control.call` переписываются на новые id. Ответ — id процесса.
pub fn import(store: &Store, flow: &Flow, now: i64) -> Result<i64, StoreError> {
    Ok(store.import_flow_bundle(flow, now)?.id)
}

pub(crate) fn remap(nodes: &mut [Node], ids: &HashMap<i64, i64>) {
    for node in nodes {
        if node.op == "control.call"
            && let Some(new) = node.config.get("process").and_then(Value::as_i64).and_then(|old| ids.get(&old))
        {
            node.config.insert("process".into(), json!(new));
        }
        remap(&mut node.nodes, ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::validate::validate;
    use crate::store::tests::memory_store;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Процесс: start → вызовы `ids` по порядку.
    fn calling(name: &str, ids: &[i64]) -> Flow {
        let mut nodes = vec![json!({ "id": "s", "op": "start" })];
        let mut edges = Vec::new();
        let mut prev = "s".to_string();
        for (i, id) in ids.iter().enumerate() {
            let node = format!("c{i}");
            nodes.push(json!({ "id": node, "op": "control.call", "config": { "process": id } }));
            edges.push(json!({ "from": prev, "to": node }));
            prev = node;
        }
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": name, "nodes": nodes, "edges": edges })).unwrap()
    }

    /// A → B → C и A → C.
    fn three(store: &Store) -> (i64, i64, i64) {
        let c = store.create_flow(&calling("C", &[]), 1).unwrap().id;
        let b = store.create_flow(&calling("B", &[c]), 1).unwrap().id;
        let a = store.create_flow(&calling("A", &[b, c]), 1).unwrap().id;
        (a, b, c)
    }

    #[test]
    fn export_bundles_called_processes_once() {
        let store = memory_store();
        let (a, b, c) = three(&store);
        let exported = export(&store, a).unwrap();
        assert_eq!(exported.subflows.keys().cloned().collect::<BTreeSet<_>>(), BTreeSet::from([b.to_string(), c.to_string()]));
        assert!(validate(&exported, &|_| None).is_ok(), "файл экспорта самодостаточен");
    }

    #[test]
    fn import_creates_records_and_rewrites_calls() {
        let source = memory_store();
        let (a, _, _) = three(&source);
        let exported = export(&source, a).unwrap();
        let target = memory_store();
        target.create_flow(&calling("Чужой", &[]), 1).unwrap();
        let id = import(&target, &exported, 5).unwrap();
        let root = target.get_flow(id).unwrap().unwrap().flow;
        assert_eq!(root.name, "A");
        assert!(root.subflows.is_empty());
        let name = |id: i64| target.get_flow(id).unwrap().unwrap().flow.name;
        let calls = called(&root);
        assert_eq!(calls.iter().map(|id| name(*id)).collect::<Vec<_>>(), ["B", "C"]);
        let inner = called(&target.get_flow(calls[0]).unwrap().unwrap().flow);
        assert_eq!(name(inner[0]), "C", "ссылки внутри подпроцессов тоже переписаны");
        let lookup = |id: i64| target.get_flow(id).ok().flatten().map(|record| record.flow);
        assert!(validate(&root, &lookup).is_ok());
    }

    #[test]
    fn failed_import_rolls_back_all_subflows_and_preserves_the_legacy_outbox() {
        let source = memory_store();
        let (a, _, _) = three(&source);
        let package = export(&source, a).unwrap();
        let target = memory_store();
        let existing = target.create_flow(&calling("Existing", &[]), 1).unwrap();
        target.raw().execute("INSERT INTO rpa_licenses(resource, license, pending, pid) VALUES (?1, 'legacy-owner', 0, 0)", [&existing.quota_id]).unwrap();
        target.raw().execute_batch("CREATE TRIGGER reject_root BEFORE INSERT ON flows WHEN NEW.name = 'A' BEGIN SELECT RAISE(ABORT, 'test write failure'); END;").unwrap();
        assert!(import(&target, &package, 2).is_err());
        assert_eq!(target.list_flows().unwrap().as_slice(), std::slice::from_ref(&existing));
        target.raw().execute_batch("DROP TRIGGER reject_root").unwrap();
        let id = import(&target, &package, 3).unwrap();
        assert_eq!(export(&target, id).unwrap().subflows.len(), 2);
        target.delete_flow(existing.id).unwrap();
        let old: (String, i64, i64) = target.raw().query_row("SELECT license, pending, pid FROM rpa_licenses WHERE resource = ?1", [&existing.quota_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!(old, ("legacy-owner".into(), 0, 0));
    }

    #[test]
    fn export_skips_missing_and_self_calls() {
        let store = memory_store();
        let a = store.create_flow(&calling("A", &[]), 1).unwrap().id;
        store.update_flow(a, &calling("A", &[a, 99]), 2).unwrap();
        assert!(export(&store, a).unwrap().subflows.is_empty(), "свой id и пропавший процесс в экспорт не попадают");
    }
}
