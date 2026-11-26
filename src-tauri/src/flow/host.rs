//! Что запуску нужно от приложения вне браузера: данные основного профиля (ключ 2FA, теги), а с New Browser — другие
//! профили. Реализация — `run::CoreHost`; в тестах — `FakeHost`.

use crate::engine::page::Browser;
use crate::flow::Busy;
use futures_util::future::BoxFuture;

pub(crate) trait Host: Send + Sync {
    /// Ключ 2FA основного профиля, как сохранён; нет — `None`. Читает Verification Code (его задача позже).
    fn totp_secret(&self) -> Result<Option<String>, String>;
    /// Теги основного профиля.
    fn profile_tags(&self) -> Result<Vec<String>, String>;
    /// Все теги приложения.
    fn tags(&self) -> Result<Vec<String>, String>;
    /// Профиль с номером `serial`: занимает его и запускает браузер или подключается к открытому (как шаг 2 запуска).
    /// С `quit` запущенный браузер и занятость профиля держит запуск до `close` или конца запуска, даже если
    /// ожидание `open` оборвали (тайм-аут, закрытие основного браузера): в конце запуска браузер гасится, затем
    /// профиль освобождается.
    fn open(&self, serial: i64, quit: bool) -> BoxFuture<'_, Result<Opened, String>>;
    /// После тела New Browser: `quit` гасит браузер, если его запустил этот узел; профиль освобождается.
    fn close(&self, opened: Opened, quit: bool) -> BoxFuture<'_, ()>;
}

/// Профиль, открытый New Browser.
pub(crate) struct Opened {
    pub(crate) profile_id: String,
    pub(crate) browser: Browser,
    /// Текущая вкладка внутри New Browser.
    pub(crate) tab: String,
    pub(crate) relay_port: u16,
    /// Браузер запустил этот узел; иначе он был открыт раньше, и его не гасим.
    pub(crate) launched: bool,
    /// Очеловечивание ввода второго профиля: тело New Browser вводит текст так, как задано у него.
    pub(crate) humanize: bool,
    /// Профиль занят, пока узел работает (поле только держит занятость). `None` — занятость держит список
    /// запущенных узлом браузеров в `CoreHost` до их остановки.
    #[allow(dead_code)]
    pub(crate) busy: Option<Busy>,
}

/// Изменения основного профиля (Update Profile Remark / Tag): запуск применяет их в конце по порядку к заметке и тегам
/// профиля на тот момент — правки из интерфейса или Local API за время запуска не теряются.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ProfileDraft {
    pub(crate) ops: Vec<ProfileOp>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileOp {
    Remark { append: bool, text: String },
    /// Теги — в написании хранилища, без повторов.
    Tags { append: bool, tags: Vec<String> },
}

impl ProfileDraft {
    /// Заметка после изменений запуска; `current` (заметка сейчас) читается, только если они есть. `None` — заметку
    /// запуск не менял. `append` — с новой строки, если заметка не пуста.
    pub(crate) fn note<E>(&self, current: impl FnOnce() -> Result<String, E>) -> Result<Option<String>, E> {
        if !self.ops.iter().any(|op| matches!(op, ProfileOp::Remark { .. })) {
            return Ok(None);
        }
        let mut note = current()?;
        for op in &self.ops {
            if let ProfileOp::Remark { append, text } = op {
                note = if !append || note.is_empty() { text.clone() } else { format!("{note}\n{text}") };
            }
        }
        Ok(Some(note))
    }

    /// Теги после изменений запуска; `current` (теги сейчас) читается, только если они есть. `None` — теги запуск не
    /// менял.
    pub(crate) fn tags<E>(&self, current: impl FnOnce() -> Result<Vec<String>, E>) -> Result<Option<Vec<String>>, E> {
        if !self.ops.iter().any(|op| matches!(op, ProfileOp::Tags { .. })) {
            return Ok(None);
        }
        let mut tags = current()?;
        for op in &self.ops {
            if let ProfileOp::Tags { append, tags: added } = op {
                if !append {
                    tags.clear();
                }
                for tag in added {
                    push_unique(&mut tags, tag);
                }
            }
        }
        Ok(Some(tags))
    }
}

/// Теги сравниваются как в хранилище: без учёта регистра и «ё» = «е».
pub(crate) fn push_unique(list: &mut Vec<String>, tag: &str) {
    if !list.iter().any(|known| crate::store::fold(known) == crate::store::fold(tag)) {
        list.push(tag.to_string());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    /// Подставное приложение: данные основного профиля — поля; `opened` — что отдаст первый `open`.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub(crate) secret: Option<String>,
        pub(crate) profile_tags: Vec<String>,
        pub(crate) tags: Vec<String>,
        pub(crate) opened: std::sync::Mutex<Option<super::Opened>>,
    }

    impl super::Host for FakeHost {
        fn totp_secret(&self) -> Result<Option<String>, String> {
            Ok(self.secret.clone())
        }

        fn profile_tags(&self) -> Result<Vec<String>, String> {
            Ok(self.profile_tags.clone())
        }

        fn tags(&self) -> Result<Vec<String>, String> {
            Ok(self.tags.clone())
        }

        fn open(&self, _: i64, _: bool) -> futures_util::future::BoxFuture<'_, Result<super::Opened, String>> {
            let opened = self.opened.lock().unwrap().take();
            Box::pin(async { opened.ok_or_else(|| "нет такого профиля".to_string()) })
        }

        fn close(&self, _: super::Opened, _: bool) -> futures_util::future::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }
}
