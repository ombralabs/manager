//! Корзина профилей (спецификация 3А): удаление откладывает профиль, восстановление возвращает, окончательное
//! удаление стирает папку и запись. Автоочистки по сроку нет — вопрос заказчику.
use super::bulk::BulkReport;
use super::profiles::{not_found, ProfileRow};
use super::{CommandError, Core};
use crate::engine::Engine;

impl<E: Engine> Core<E> {
    pub fn list_trash(&self) -> Result<Vec<ProfileRow>, CommandError> {
        let records = self.store.list_trash()?;
        self.trash_rows(records)
    }

    /// Снимает `deleted_at`; удалённая группа → «без группы» (триггер 002 `groups_unlink_profiles` уже сделал это
    /// в базе). Замок простоя — чтобы не разминуться с одновременным окончательным удалением того же id.
    pub async fn restore_profiles(&self, ids: &[String]) -> BulkReport {
        let mut report = BulkReport::default();
        for id in ids {
            let result = async {
                let _idle = self.supervisor.lock_idle(id).await?;
                match self.store.get_profile_any(id)? {
                    Some(rec) if rec.deleted_at.is_some() => self.store.set_deleted(std::slice::from_ref(id), None).map_err(Into::into),
                    _ => Err(not_found()),
                }
            }
            .await;
            report.push(id.clone(), result);
        }
        report
    }

    /// Стирает папку и запись — то, что раньше делало удаление. Замок простоя не даёт гонке с импортом или клоном того же id.
    pub async fn purge_profiles(&self, ids: &[String]) -> BulkReport {
        let mut report = BulkReport::default();
        for id in ids {
            let result = async {
                let _idle = self.supervisor.lock_idle(id).await?;
                match self.store.get_profile_any(id)? {
                    Some(rec) if rec.deleted_at.is_some() => {}
                    _ => return Err(not_found()),
                }
                // Удаление большой папки не на воркере tokio: замок простоя держится через await.
                let (paths, owned) = (self.paths.clone(), id.clone());
                tokio::task::spawn_blocking(move || Self::remove_profile_files(&paths, &owned))
                    .await
                    .map_err(|e| CommandError::new("Internal", format!("фоновая задача прервана: {e}")))??;
                self.store.delete_profile(id)?;
                self.supervisor.forget(id);
                Ok(())
            }
            .await;
            report.push(id.clone(), result);
        }
        report
    }

    pub async fn empty_trash(&self) -> Result<BulkReport, CommandError> {
        let ids: Vec<String> = self.store.list_trash()?.into_iter().map(|p| p.id).collect();
        Ok(self.purge_profiles(&ids).await)
    }
}
