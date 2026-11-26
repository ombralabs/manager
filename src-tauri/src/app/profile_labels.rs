use super::{CommandError, Core};
use crate::engine::Engine;
use crate::store::profile_labels::ProfileLabel;
use crate::supervisor::Event;

impl<E: Engine> Core<E> {
    pub fn list_profile_labels(&self) -> Result<Vec<ProfileLabel>, CommandError> {
        Ok(self.store.list_profile_labels()?)
    }

    pub fn save_profile_label(&self, id: Option<i64>, name: &str, color: &str) -> Result<ProfileLabel, CommandError> {
        let saved = self.store.save_profile_label(id, name, color)?;
        self.supervisor.emit(Event::ProfilesChanged);
        Ok(saved)
    }

    pub fn delete_profile_label(&self, id: i64) -> Result<(), CommandError> {
        self.store.delete_profile_label(id)?;
        self.supervisor.emit(Event::ProfilesChanged);
        Ok(())
    }

    pub fn set_profile_label(&self, id: &str, label_id: Option<i64>) -> Result<Option<ProfileLabel>, CommandError> {
        let saved = self.store.set_profile_label(id, label_id)?;
        self.supervisor.emit(Event::ProfilesChanged);
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::profiles::ListQueryDto;
    use crate::app::tests::{create_with_proxy, env};

    #[tokio::test]
    async fn manual_status_filter_uses_label_ids_and_does_not_depend_on_browser_state() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let c = create_with_proxy(&e.core, "C", e.proxy_port, 3);
        let label = e.core.save_profile_label(None, "Проверить", "#2447c9").unwrap();
        for id in [&a.id, &b.id] { e.core.set_profile_label(id, Some(label.id)).unwrap(); }
        e.core.supervisor.start(&a.id).await.unwrap();
        let ids = |label_id| e.core.list_profiles(&ListQueryDto { label_id, ..Default::default() }).unwrap().rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
        assert_eq!(ids(Some(label.id)), [a.id.clone(), b.id.clone()]);
        assert_eq!(ids(Some(0)).as_slice(), std::slice::from_ref(&c.id));
        assert_eq!(ids(None).len(), 3);
        let csv = e.core.export_csv(&ListQueryDto { label_id: Some(label.id), ..Default::default() }, false).unwrap();
        assert_eq!(::csv::Reader::from_reader(csv.as_bytes()).records().count(), 2, "экспорт использует тот же фильтр");
        e.core.save_profile_label(Some(label.id), "Готов", "#237a36").unwrap();
        assert_eq!(ids(Some(label.id)), [a.id.clone(), b.id.clone()], "переименование сохраняет фильтр");
        e.core.delete_profile_label(label.id).unwrap();
        assert_eq!(ids(Some(0)).len(), 3, "удаление снимает статус с профилей");
        e.core.supervisor.stop(&a.id).await.unwrap();
    }

    #[tokio::test]
    async fn manual_status_is_independent_of_the_browser_and_profile_edits() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Магазин", e.proxy_port, 1);
        let ready = e.core.list_profile_labels().unwrap().into_iter().find(|label| label.preset.as_deref() == Some("ready")).unwrap();
        e.core.set_profile_label(&profile.id, Some(ready.id)).unwrap();
        e.core.supervisor.start(&profile.id).await.unwrap();
        let list = e.core.list_profiles(&ListQueryDto::default()).unwrap();
        assert_eq!(list.rows[0].label, Some(ready.clone()));
        assert!(matches!(list.rows[0].status, crate::supervisor::ProfileStatus::Running(_)));
        e.core.supervisor.stop(&profile.id).await.unwrap();
        let list = e.core.list_profiles(&ListQueryDto::default()).unwrap();
        assert_eq!(list.rows[0].label, Some(ready));
        e.core.set_profile_label(&profile.id, None).unwrap();
        assert!(e.core.list_profiles(&ListQueryDto::default()).unwrap().rows[0].label.is_none());
    }
}
