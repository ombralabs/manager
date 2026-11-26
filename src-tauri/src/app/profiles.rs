//! Список, редактор и удаление профилей на уровне приложения.
use super::forms::{ProfileForm, clean_lines, profile_input, validate_form};
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::fingerprint::validate::DEVICE_MEMORY;
use crate::fingerprint::{self, Arch, Catalog, FieldError, Fingerprint, Gpu, OsRandom, Screen, generate, regenerate};
use crate::paths::remove_path;
use crate::store::bookmarks::Bookmark;
use crate::store::groups::GroupRecord;
use crate::store::profiles::{GroupFilter, ListQuery, ProfileRecord, SortKey};
use crate::store::proxies::{ProxyKind, ProxyRecord};
use crate::supervisor::{ProfileStatus, StatusFilter, pidfile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListQueryDto {
    pub search: String,
    pub sort: SortKey,
    pub descending: bool,
    /// Техническое состояние браузера; пользовательские статусы фильтруются через `label_id`.
    pub status: Option<StatusFilter>,
    /// Ручная метка: None — любая, 0 — без метки, положительное число — id справочника.
    pub label_id: Option<i64>,
    pub group_id: Option<i64>,
    /// «Без группы»; важнее `group_id`.
    pub ungrouped: bool,
    pub tag: Option<String>,
}

/// Колонка «Группа»: название и цвет.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupView {
    pub id: i64,
    pub name: String,
    pub color: String,
}

/// Колонка «Шлюз»: адрес прокси и итог его последней проверки — кнопкой «Проверить» или при запуске.
#[derive(Debug, Clone, Serialize)]
pub struct GatewayView {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub ip: Option<String>,
    pub country: Option<String>,
    /// `None` — прокси ещё не проверяли.
    pub ok: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileRow {
    pub direct: bool,
    pub id: String,
    pub serial: i64,
    pub name: String,
    pub note: String,
    pub group: Option<GroupView>,
    pub tags: Vec<String>,
    pub label: Option<crate::store::profile_labels::ProfileLabel>,
    pub gateway: Option<GatewayView>,
    /// Шлюз заменён при последнем запуске — подпись в колонке «Шлюз».
    pub gateway_replaced_at: Option<i64>,
    pub restore_tabs: bool,
    pub os_version: String,
    pub brand: String,
    pub seed: u32,
    pub status: ProfileStatus,
    pub created_at: i64,
    pub last_opened_at: Option<i64>,
    /// Когда профиль попал в корзину; у живых `None`.
    pub deleted_at: Option<i64>,
    /// Набор расширений: раздел «Расширения» в панели группы.
    pub ext_category_id: i64,
}

/// Справочники для строк списка: читаются один раз на весь список, а не на каждую строку.
struct RowLookups {
    proxies: HashMap<i64, ProxyRecord>,
    groups: HashMap<i64, GroupRecord>,
    tags: HashMap<String, Vec<String>>,
    labels: HashMap<String, crate::store::profile_labels::ProfileLabel>,
}

fn row(record: ProfileRecord, status: ProfileStatus, lookups: &RowLookups) -> ProfileRow {
    // Страна и IP — из последней проверки прокси: её пишет и «Проверить», и каждый запуск.
    let active_proxy = match &status {
        ProfileStatus::Running(info) | ProfileStatus::Paused(info) | ProfileStatus::Waiting { info: Some(info), .. } => {
            (info.proxy_id > 0).then_some(info.proxy_id)
        }
        _ => record.proxy_id,
    };
    let gateway = active_proxy.and_then(|id| lookups.proxies.get(&id)).map(|p| GatewayView {
        kind: p.kind,
        host: p.host.clone(),
        port: p.port,
        ip: p.check.as_ref().and_then(|c| c.exit_ip.clone()),
        country: p.check.as_ref().and_then(|c| c.country.clone()),
        ok: p.check.as_ref().map(|c| c.ok),
    });
    let group = record
        .group_id
        .and_then(|id| lookups.groups.get(&id))
        .map(|g| GroupView { id: g.id, name: g.name.clone(), color: g.color.clone() });
    let tags = lookups.tags.get(&record.id).cloned().unwrap_or_default();
    let label = lookups.labels.get(&record.id).cloned();
    let fingerprint = Fingerprint::from_json(&record.fingerprint_json).ok();
    ProfileRow {
        direct: record.network.direct,
        id: record.id,
        serial: record.serial,
        name: record.name,
        note: record.note,
        group,
        tags,
        label,
        gateway,
        gateway_replaced_at: record.gateway_replaced_at,
        restore_tabs: record.restore_tabs,
        os_version: fingerprint.as_ref().map(|f| f.os_version.clone()).unwrap_or_default(),
        brand: fingerprint.as_ref().map(|f| f.brand.clone()).unwrap_or_default(),
        seed: fingerprint.as_ref().map(|f| f.seed).unwrap_or(0),
        status,
        created_at: record.created_at,
        last_opened_at: record.last_opened_at,
        deleted_at: record.deleted_at,
        ext_category_id: record.ext_category_id,
    }
}

/// Строки после поиска и фильтра; `total` и `running` — по всем профилям.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileList {
    pub rows: Vec<ProfileRow>,
    pub total: i64,
    pub running: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileDetails {
    pub profile: ProfileRecord,
    pub fingerprint: Fingerprint,
    pub proxy: Option<ProxyRecord>,
    /// Теги профиля — для поля «Теги» редактора.
    pub tags: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    /// Почерк профиля — персона движка из его seed (раздел «Очеловечивание» редактора).
    pub persona: crate::humanize::PersonaView,
}

/// Значения для списков редактора отпечатка — из набора комбинаций под архитектуру машины.
#[derive(Debug, Clone, Serialize)]
pub struct FingerprintOptions {
    pub screens: Vec<Screen>,
    pub gpus: Vec<Gpu>,
    pub hardware_concurrency: Vec<u32>,
    pub device_memory: Vec<u32>,
    pub engine_version: Option<String>,
}

pub(super) fn not_found() -> CommandError {
    CommandError::new("NotFound", "профиль не найден")
}

impl ProfileRecord {
    /// Отпечаток из сохранённого JSON; повреждённый — ошибка `BadFingerprint`.
    pub fn fingerprint(&self) -> Result<Fingerprint, CommandError> {
        Fingerprint::from_json(&self.fingerprint_json)
            .map_err(|_| CommandError::new("BadFingerprint", "отпечаток профиля повреждён"))
    }
}

pub fn fingerprint_options(
    catalog: &Catalog,
    arch: Arch,
    engine_version: Option<String>,
) -> FingerprintOptions {
    let combos: Vec<_> = catalog.combos.iter().filter(|c| c.arch == arch.catalog_name()).collect();
    let mut screens: Vec<Screen> = combos.iter().map(|c| c.screen).collect();
    screens.sort_by_key(|s| (s.width * s.height, s.width));
    screens.dedup();
    let mut gpus: Vec<Gpu> = combos.iter().map(|c| c.gpu.clone()).collect();
    gpus.sort_by(|a, b| a.renderer.cmp(&b.renderer));
    gpus.dedup();
    let mut cores: Vec<u32> = combos.iter().map(|c| c.hardware_concurrency).collect();
    cores.sort_unstable();
    cores.dedup();
    FingerprintOptions {
        screens,
        gpus,
        hardware_concurrency: cores,
        device_memory: DEVICE_MEMORY.to_vec(),
        engine_version,
    }
}

impl<E: Engine> Core<E> {
    pub fn list_profiles(&self, query: &ListQueryDto) -> Result<ProfileList, CommandError> {
        let search = Some(query.search.clone()).filter(|s| !s.trim().is_empty());
        let group = if query.ungrouped { Some(GroupFilter::Ungrouped) } else { query.group_id.map(GroupFilter::Id) };
        let tag = query.tag.clone().filter(|t| !t.trim().is_empty());
        let records =
            self.store.list_profiles(&ListQuery { search, sort: query.sort, descending: query.descending, group, tag })?;
        let lookups = self.row_lookups()?;
        let rows = records
            .into_iter()
            .filter_map(|record| {
                if let Some(label_id) = query.label_id {
                    let assigned = lookups.labels.get(&record.id).map(|label| label.id);
                    if (label_id == 0 && assigned.is_some()) || (label_id != 0 && assigned != Some(label_id)) { return None; }
                }
                let status = self.supervisor.status(&record.id);
                if query.status.is_some_and(|filter| !filter.matches(&status)) {
                    return None;
                }
                Some(row(record, status, &lookups))
            })
            .collect();
        Ok(ProfileList { rows, total: self.store.count_profiles()?, running: self.supervisor.active_count() })
    }

    /// Справочники для строк списка (живого и корзины).
    fn row_lookups(&self) -> Result<RowLookups, CommandError> {
        Ok(RowLookups {
            proxies: self.store.list_proxies()?.into_iter().map(|p| (p.id, p)).collect(),
            groups: self.store.list_groups()?.into_iter().map(|g| (g.id, g)).collect(),
            tags: self.store.tags_by_profile()?,
            labels: self.store.labels_by_profile()?,
        })
    }

    /// Строки корзины: статус у них всегда «остановлен», фильтров нет.
    pub(super) fn trash_rows(&self, records: Vec<ProfileRecord>) -> Result<Vec<ProfileRow>, CommandError> {
        let lookups = self.row_lookups()?;
        Ok(records
            .into_iter()
            .map(|record| {
                let status = self.supervisor.status(&record.id);
                row(record, status, &lookups)
            })
            .collect())
    }

    pub fn profile_details(&self, id: &str) -> Result<ProfileDetails, CommandError> {
        let profile = self.store.get_profile(id)?.ok_or_else(not_found)?;
        let fingerprint = profile.fingerprint()?;
        let proxy = profile.proxy_id.map(|pid| self.store.get_proxy(pid)).transpose()?.flatten();
        let tags = self.store.profile_tags(id)?;
        let bookmarks = self.store.profile_bookmarks(id)?;
        let persona = crate::humanize::persona(profile.humanize_seed, fingerprint.os);
        Ok(ProfileDetails { profile, fingerprint, proxy, tags, bookmarks, persona })
    }

    /// «Сбросить персону»: новый seed — почерк профиля меняется (ТЗ 7.5); запущенный профиль получит его со следующего
    /// запуска.
    pub fn reset_persona(&self, id: &str) -> Result<ProfileDetails, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        self.store.set_humanize_seed(id, crate::humanize::new_seed()?)?;
        self.profile_details(id)
    }

    pub fn create_profile(&self, form: &ProfileForm, now: i64) -> Result<ProfileRecord, CommandError> {
        let errors = self.profile_form_errors(form, None)?;
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let tags = form.tags.as_deref().map(clean_lines).unwrap_or_default();
        // Профиль и его теги — одной транзакцией.
        let input = self.profile_input_with_connection(form, None)?;
        let mut created = self.store.create_profiles(std::slice::from_ref(&input), &tags, now)?;
        Ok(created.remove(0))
    }

    /// Сохранение редактора. Прокси — запись общего списка по `proxy_id`; сама запись здесь не правится
    /// (её правит диалог прокси). Без поля `proxy_id` закрепление остаётся прежним.
    pub fn update_profile(&self, id: &str, form: &ProfileForm) -> Result<ProfileRecord, CommandError> {
        let existing = self.store.get_profile(id)?.ok_or_else(not_found)?;
        let errors = self.profile_form_errors(form, Some(&existing))?;
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        // Профиль и теги — одной транзакцией: сбой тегов не оставляет сохранённого профиля.
        let tags = form.tags.as_deref().map(clean_lines);
        let input = self.profile_input_with_connection(form, Some(&existing))?;
        Ok(self.store.update_profile_with_tags(id, &input, tags.as_deref())?)
    }

    fn profile_input_with_connection(&self, form: &ProfileForm, existing: Option<&ProfileRecord>) -> Result<crate::store::profiles::ProfileInput, CommandError> {
        let mut input = profile_input(form, existing);
        if input.network.as_ref().is_some_and(|network| network.direct) { input.proxy_id = None; }
        if let Some(proxy) = &mut input.inline_proxy {
            if matches!(proxy.password, crate::store::secrets::SecretField::Keep) {
                proxy.password = form.inline_proxy_source.or_else(|| existing.and_then(|profile| profile.proxy_id)).map(|id| self.store.proxy_credentials(id)).transpose()?.and_then(|credentials| credentials.password)
                    .map_or(crate::store::secrets::SecretField::Clear, crate::store::secrets::SecretField::Set);
            }
            input.proxy_id = None;
        }
        Ok(input)
    }

    /// Ошибки формы и неизвестный прокси — одним списком, до записи в базу.
    fn profile_form_errors(&self, form: &ProfileForm, existing: Option<&ProfileRecord>) -> Result<Vec<FieldError>, CommandError> {
        let mut errors = validate_form(form, Arch::host());
        if let Some(proxy) = &form.inline_proxy {
            let has_password = form.inline_proxy_source.or_else(|| existing.and_then(|record| record.proxy_id)).map(|id| self.store.get_proxy(id)).transpose()?.flatten().is_some_and(|proxy| proxy.has_password);
            errors.extend(super::forms::validate_proxy_form(proxy, has_password));
        }
        if form.network.as_ref().is_some_and(|network| network.direct) && (form.proxy_id.flatten().is_some() || form.inline_proxy.is_some()) {
            errors.push(FieldError { field: "proxy_id", code: "invalid" });
        }
        if let Some(Some(proxy_id)) = form.proxy_id
            && self.store.get_proxy(proxy_id)?.is_none()
        {
            errors.push(FieldError { field: "proxy_id", code: "invalid" });
        }
        Ok(errors)
    }

    /// Запущенный профиль не удаляется. Удаление — в корзину (`deleted_at`): запись и папка остаются до
    /// окончательного удаления (`purge_profiles`); прокси остаётся в общем списке.
    pub async fn delete_profile(&self, id: &str) -> Result<(), CommandError> {
        let _idle = self.supervisor.lock_idle(id).await?;
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        self.store.set_deleted(std::slice::from_ref(&id.to_string()), Some(crate::sys::unix_now()))?;
        self.supervisor.forget(id);
        Ok(())
    }

    /// Пачка Local API (спецификация 2В-3, «Пачки при гонке с запуском»): замки простоя всех профилей, затем проверка
    /// и перенос в корзину — всё или ничего. Запуск в это время ждёт очереди и после удаления получает `NotFound`.
    pub async fn delete_profiles_atomic(&self, ids: &[String]) -> Result<(), CommandError> {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let _idle = self.supervisor.lock_idle_many(&ids).await?;
        for id in &ids {
            self.store.get_profile(id)?.ok_or_else(not_found)?;
        }
        self.store.set_deleted(&ids, Some(crate::sys::unix_now()))?;
        for id in &ids {
            self.supervisor.forget(id);
        }
        Ok(())
    }

    /// Папка браузера, кэш Chromium, журналы прослойки и браузера, PID-файл.
    pub(super) fn remove_profile_files(paths: &crate::paths::Paths, id: &str) -> Result<(), CommandError> {
        remove_path(&paths.profile_dir(id))?;
        if let Some(cache) = paths.chromium_cache_dir(id) {
            remove_path(&cache)?;
        }
        let relay_log = paths.relay_log(id);
        for path in [relay_log.with_extension("jsonl.1"), relay_log, paths.chromium_log(id)] {
            remove_path(&path)?;
        }
        pidfile::remove(&paths.run_dir(), id)?;
        Ok(())
    }

    /// Очистка кэша остановленного профиля: папки кэша страниц, кода, GPU и шейдеров, включая вынесенный в `~/Library/Caches`.
    /// Cookies в базе, хранилище сайтов и история не трогаются.
    pub async fn clear_cache(&self, id: &str) -> Result<(), CommandError> {
        let _idle = self.supervisor.lock_idle(id).await?;
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        for path in self.paths.cache_paths(id) {
            remove_path(&path)?;
        }
        Ok(())
    }

    pub fn new_fingerprint(&self) -> Result<Fingerprint, CommandError> {
        Ok(generate(Catalog::builtin(), Arch::host(), &mut OsRandom)?)
    }

    pub fn regenerate_fingerprint(&self, current: &Fingerprint) -> Result<Fingerprint, CommandError> {
        Ok(regenerate(current, Catalog::builtin(), Arch::host(), &mut OsRandom)?)
    }

    pub fn validate_fingerprint(&self, fingerprint: &Fingerprint) -> Vec<FieldError> {
        fingerprint::validate(fingerprint, Arch::host())
    }

    pub fn fingerprint_options(&self) -> FingerprintOptions {
        fingerprint_options(
            Catalog::builtin(),
            Arch::host(),
            self.engine.installed().map(|b| b.version),
        )
    }
}
