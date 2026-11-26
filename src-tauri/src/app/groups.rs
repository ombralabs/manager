//! Группы и теги на уровне приложения (спецификация 2А, «Группы, теги, клоны, массовые операции»).

use super::{CommandError, Core};
use crate::engine::Engine;
use crate::store::groups::{GroupInput, GroupRecord};
use crate::store::tags::TagRecord;
use serde::{Deserialize, Serialize};

/// Группа в форме экрана «Группы». Проверку имени и цвета делает хранилище (`group_name`, `group_color`).
#[derive(Debug, Clone, Deserialize)]
pub struct GroupForm {
    pub name: String,
    pub color: String,
    pub note: String,
    pub quick_access: bool,
}

impl GroupForm {
    fn input(&self) -> GroupInput {
        GroupInput {
            name: self.name.trim().to_string(),
            color: self.color.trim().to_string(),
            note: self.note.trim().to_string(),
            quick_access: self.quick_access,
        }
    }
}

/// Группы с числом профилей и сколько профилей «Без группы» — для «облака» над списком.
#[derive(Debug, Clone, Serialize)]
pub struct GroupList {
    pub groups: Vec<GroupRecord>,
    pub ungrouped: i64,
}

impl<E: Engine> Core<E> {
    pub fn list_groups(&self) -> Result<GroupList, CommandError> {
        Ok(GroupList { groups: self.store.list_groups()?, ungrouped: self.store.ungrouped_count()? })
    }

    /// Такое же имя без учёта регистра → `GroupExists`.
    pub fn create_group(&self, form: &GroupForm, now: i64) -> Result<GroupRecord, CommandError> {
        Ok(self.store.create_group(&form.input(), now)?)
    }

    pub fn update_group(&self, id: i64, form: &GroupForm) -> Result<GroupRecord, CommandError> {
        Ok(self.store.update_group(id, &form.input())?)
    }

    /// Профили группы переходят в «Без группы».
    pub fn delete_group(&self, id: i64) -> Result<(), CommandError> {
        Ok(self.store.delete_group(id)?)
    }

    /// Все теги с числом профилей — для подсказок и фильтра.
    pub fn list_tags(&self) -> Result<Vec<TagRecord>, CommandError> {
        Ok(self.store.list_tags()?)
    }

    pub fn change_profile_tags(&self, id: &str, add: &[String], remove: &[String]) -> Result<Vec<String>, CommandError> {
        let tags = self.store.change_profile_tags(id, add, remove)?;
        self.supervisor.emit(crate::supervisor::Event::ProfilesChanged);
        Ok(tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{env, form};

    fn group(name: &str) -> GroupForm {
        GroupForm { name: name.into(), color: "#2447C9".into(), note: String::new(), quick_access: false }
    }

    #[tokio::test]
    async fn groups_are_managed_and_names_are_unique() {
        let e = env().await;
        let shops = e.core.create_group(&group("  Магазины "), 10).unwrap();
        assert_eq!((shops.name.as_str(), shops.color.as_str(), shops.created_at), ("Магазины", "#2447c9", 10));
        assert_eq!(e.core.create_group(&group("МАГАЗИНЫ"), 11).unwrap_err().code, "GroupExists");
        let mut bad = group("Реклама");
        bad.color = "синий".into();
        assert_eq!(e.core.create_group(&bad, 11).unwrap_err().fields.unwrap()[0].field, "group_color");
        assert_eq!(e.core.create_group(&group(&"я".repeat(51)), 11).unwrap_err().fields.unwrap()[0].field, "group_name");

        let mut renamed = group("Магазины 2");
        renamed.quick_access = true;
        let updated = e.core.update_group(shops.id, &renamed).unwrap();
        assert_eq!((updated.name.as_str(), updated.quick_access), ("Магазины 2", true));
        assert_eq!(e.core.update_group(999, &renamed).unwrap_err().code, "NotFound");

        let mut in_group = form("a", None);
        in_group.group_id = Some(Some(shops.id));
        let rec = e.core.create_profile(&in_group, 1).unwrap();
        e.core.create_profile(&form("b", None), 2).unwrap();
        let list = e.core.list_groups().unwrap();
        assert_eq!((list.groups.len(), list.groups[0].profile_count, list.ungrouped), (1, 1, 1));

        e.core.delete_group(shops.id).unwrap();
        assert_eq!(e.core.store.get_profile(&rec.id).unwrap().unwrap().group_id, None, "профиль ушёл в «Без группы»");
        assert_eq!(e.core.list_groups().unwrap().ungrouped, 2);
        assert_eq!(e.core.delete_group(shops.id).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn tags_are_listed_with_profile_counts() {
        let e = env().await;
        let a = e.core.create_profile(&form("a", None), 1).unwrap();
        let b = e.core.create_profile(&form("b", None), 2).unwrap();
        e.core.store.add_tags(&[a.id.clone(), b.id.clone()], &["Shop".into()]).unwrap();
        e.core.store.add_tags(std::slice::from_ref(&b.id), &["vip".into()]).unwrap();
        let tags: Vec<(String, i64)> = e.core.list_tags().unwrap().into_iter().map(|t| (t.name, t.profile_count)).collect();
        assert_eq!(tags, [("Shop".to_string(), 2), ("vip".to_string(), 1)]);
        assert_eq!(e.core.change_profile_tags(&a.id, &["VIP".into(), "new".into()], &["SHOP".into()]).unwrap(), ["new", "vip"]);
        let tags: Vec<(String, i64)> = e.core.list_tags().unwrap().into_iter().map(|t| (t.name, t.profile_count)).collect();
        assert_eq!(tags, [("new".to_string(), 1), ("Shop".to_string(), 1), ("vip".to_string(), 2)]);
        assert_eq!(e.core.list_profiles(&Default::default()).unwrap().rows[0].tags, ["new", "vip"]);
    }
}
