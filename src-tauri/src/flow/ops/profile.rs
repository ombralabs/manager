//! Profile Information: Update Profile Remark и Update Profile Tag (спецификация автоматизации, «Получение и
//! обработка данных»). Изменения копятся в запуске и применяются к основному профилю в конце (`run`).
use crate::flow::host::{Host, ProfileOp, push_unique};
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::vars::to_text;
use crate::store::fold;
use std::sync::Arc;

/// Больше тегов у профиля операция не оставляет (документация AdsPower: «more than 30 tags»).
pub(crate) const TAGS_LIMIT: usize = 30;

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let host = host(ctx)?;
    let append = cfg.string("mode") != "replace";
    let note = match id {
        "profile.remark" => {
            let text = cfg.text(&ctx.scope, "text")?;
            ctx.profile.ops.push(ProfileOp::Remark { append, text });
            "заметка запишется в конце запуска".to_string()
        }
        "profile.tags" => {
            let known = host.tags()?;
            let mut wanted = Vec::new();
            for raw in cfg.raw("tags").as_array().into_iter().flatten() {
                let name = ctx.scope.render(raw.as_str().unwrap_or_default())?;
                let tag = known
                    .iter()
                    .find(|tag| fold(tag) == fold(name.trim()))
                    .ok_or_else(|| OpError::Failed(format!("нет тега {}", to_text(raw))))?;
                push_unique(&mut wanted, tag);
            }
            // Предел — по тегам профиля сейчас; в конце запуска он проверяется ещё раз, по тегам того времени.
            let mut draft = ctx.profile.clone();
            draft.ops.push(ProfileOp::Tags { append, tags: wanted });
            if draft.tags(|| host.profile_tags())?.is_some_and(|tags| tags.len() > TAGS_LIMIT) {
                return Err(OpError::Failed(format!("у профиля будет больше {TAGS_LIMIT} тегов")));
            }
            ctx.profile = draft;
            "теги запишутся в конце запуска".to_string()
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), note))
}

/// Приложение запуска; без него — ошибка узла.
pub(crate) fn host(ctx: &Ctx) -> Result<Arc<dyn Host>, OpError> {
    ctx.host.clone().ok_or_else(|| OpError::Failed("у запуска нет профиля".into()))
}

#[cfg(test)]
mod tests {
    use crate::flow::host::tests::FakeHost;
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use serde_json::json;
    use std::sync::Arc;

    fn chain(nodes: serde_json::Value) -> Flow {
        let mut all = vec![json!({ "id": "s", "op": "start" })];
        let mut edges = Vec::new();
        let mut prev = "s".to_string();
        for node in nodes.as_array().unwrap() {
            let id = node["id"].as_str().unwrap().to_string();
            edges.push(json!({ "from": prev, "to": id }));
            prev = id;
            all.push(node.clone());
        }
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "t", "nodes": all, "edges": edges })).unwrap()
    }

    fn ctx(host: FakeHost) -> Ctx {
        let mut ctx = Ctx::bare();
        ctx.host = Some(Arc::new(host));
        ctx
    }

    /// Заметка после изменений запуска, если сейчас у профиля `current`.
    fn note(c: &Ctx, current: &str) -> Option<String> {
        c.profile.note(|| Ok::<_, ()>(current.to_string())).unwrap()
    }

    #[tokio::test]
    async fn remark_appends_on_a_new_line_and_replace_overwrites() {
        let mut c = ctx(FakeHost::default());
        assert_eq!(note(&c, "старое"), None, "заметку не меняли");
        c.run(&chain(json!([{ "id": "a", "op": "profile.remark", "config": { "text": "новое" } }]))).await.unwrap();
        assert_eq!(note(&c, "старое").as_deref(), Some("старое\nновое"));
        assert_eq!(note(&c, "").as_deref(), Some("новое"), "к пустой заметке — без перевода строки");
        c.run(&chain(json!([{ "id": "b", "op": "profile.remark", "config": { "mode": "replace", "text": "одно" } }]))).await.unwrap();
        assert_eq!(note(&c, "старое").as_deref(), Some("одно"));
        c.run(&chain(json!([{ "id": "c", "op": "profile.remark", "config": { "text": "два" } }]))).await.unwrap();
        assert_eq!(note(&c, "старое").as_deref(), Some("одно\nдва"), "по порядку");
    }

    fn tags(c: &Ctx, current: &[&str]) -> Option<Vec<String>> {
        c.profile.tags(|| Ok::<_, ()>(current.iter().map(|tag| tag.to_string()).collect())).unwrap()
    }

    #[tokio::test]
    async fn tags_must_exist_keep_their_spelling_and_stay_within_the_limit() {
        let known: Vec<String> = (0..31).map(|i| format!("t{i}")).chain(["VIP".to_string()]).collect();
        let mut c = ctx(FakeHost { profile_tags: vec!["t0".into()], tags: known.clone(), ..Default::default() });
        c.run(&chain(json!([{ "id": "a", "op": "profile.tags", "config": { "tags": ["vip", "T0"] } }]))).await.unwrap();
        assert_eq!(tags(&c, &["t0"]), Some(vec!["t0".to_string(), "VIP".to_string()]), "написание — как у тега, без повторов");
        let missing = c.run(&chain(json!([{ "id": "b", "op": "profile.tags", "config": { "tags": ["Нет"] } }]))).await;
        assert!(matches!(missing, Err(Fail::Node { ref message, .. }) if message == "Update Profile Tag: нет тега Нет"), "{missing:?}");
        let all: Vec<String> = known.iter().take(31).cloned().collect();
        let over = c.run(&chain(json!([{ "id": "c", "op": "profile.tags", "config": { "mode": "replace", "tags": all } }]))).await;
        assert!(matches!(over, Err(Fail::Node { ref message, .. }) if message == "Update Profile Tag: у профиля будет больше 30 тегов"), "{over:?}");
        assert_eq!(tags(&c, &["t0"]), Some(vec!["t0".to_string(), "VIP".to_string()]), "узел с ошибкой ничего не копит");
        let mut yo = ctx(FakeHost { tags: vec!["Елка".into()], ..Default::default() });
        yo.run(&chain(json!([{ "id": "a", "op": "profile.tags", "config": { "tags": ["ёлка"] } }]))).await.unwrap();
        assert_eq!(tags(&yo, &[]), Some(vec!["Елка".to_string()]), "ё = е, как в хранилище");
        let mut bare = Ctx::bare();
        let none = bare.run(&chain(json!([{ "id": "a", "op": "profile.remark", "config": { "text": "x" } }]))).await;
        assert!(matches!(none, Err(Fail::Node { ref message, .. }) if message == "Update Profile Remark: у запуска нет профиля"), "{none:?}");
    }
}
