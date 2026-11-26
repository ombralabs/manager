//! Ключ сервисного аккаунта Google и токен доступа для Sheets (спецификация А4, «Решения» и «А4-2»).
//! Ключ (JSON целиком) хранится зашифрованным в настройках; токен — в памяти процесса, по одному на аккаунт и область.
use crate::flow::services;
use crate::store::{GOOGLE_KEY_KEY, Store};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Запрос токена идёт только сюда; `token_uri` из файла ключа — лишь `aud` в JWT, чтобы подложенный файл не увёл запрос.
#[cfg_attr(test, allow(dead_code))]
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Срок JWT-заявки.
const JWT_TTL: u64 = 3600;
/// Токен считается негодным за минуту до конца срока.
const MARGIN: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
struct Key {
    r#type: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

/// (адрес аккаунта, область) → токен и момент, до которого он годен — по настенным часам: монотонные (`Instant`) на
/// macOS стоят, пока компьютер спит, и после ночи истёкший токен казался бы годным.
static CACHE: LazyLock<Mutex<Tokens>> = LazyLock::new(Default::default);
type Tokens = HashMap<(String, String), (String, SystemTime)>;

fn cache() -> std::sync::MutexGuard<'static, Tokens> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Разбор и проверка; тексты ошибок не содержат содержимого ключа.
fn parse(json: &str) -> Result<(Key, RsaKeyPair), String> {
    let key: Key = serde_json::from_str(json).map_err(|_| "Выберите JSON-файл ключа сервисного аккаунта Google".to_string())?;
    if key.r#type != "service_account" {
        return Err({
            log::warn!("Поле type должно иметь значение service_account.");
            "Выбранный файл не является ключом сервисного аккаунта Google"
        }.into());
    }
    if key.client_email.trim().is_empty() || key.token_uri.trim().is_empty() {
        return Err({
            log::warn!("Не заполнены client_email или token_uri.");
            "В файле ключа Google отсутствуют обязательные данные"
        }.into());
    }
    let pair = pem_pair(&key.private_key)?;
    Ok((key, pair))
}

fn pem_pair(pem: &str) -> Result<RsaKeyPair, String> {
    let bad = || {
        log::warn!("Поле private_key должно содержать RSA-ключ PKCS#8 в формате PEM.");
        "Файл ключа Google имеет неподдерживаемый формат. Выберите исходный JSON-файл сервисного аккаунта."
    }.to_string();
    let body: String = pem.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("-----")).collect();
    let der = B64.decode(body).map_err(|_| bad())?;
    RsaKeyPair::from_pkcs8(&der).map_err(|_| bad())
}

/// Сохраняет ключ зашифрованным; возвращает адрес аккаунта. Негодный ключ не сохраняется.
pub fn set_key(store: &Store, json: &str) -> Result<String, String> {
    let (key, _) = parse(json)?;
    store.set_secret_setting(GOOGLE_KEY_KEY, Some(json)).map_err(|e| e.to_string())?;
    cache().retain(|(email, _), _| *email != key.client_email);
    Ok(key.client_email)
}

pub fn clear_key(store: &Store) -> Result<(), String> {
    store.set_secret_setting(GOOGLE_KEY_KEY, None).map_err(|e| e.to_string())?;
    cache().clear();
    Ok(())
}

/// Сохранённый ключ как есть; `None` — не загружен.
fn stored(store: &Store) -> Result<Option<String>, String> {
    Ok(store.secret_setting(GOOGLE_KEY_KEY).map_err(|e| e.to_string())?.map(|s| s.expose().to_string()))
}

/// Адрес сервисного аккаунта для подсказки в настройках; `None` — ключ не загружен.
pub fn key_email(store: &Store) -> Result<Option<String>, String> {
    let Some(json) = stored(store)? else { return Ok(None) };
    Ok(Some(parse(&json)?.0.client_email))
}

/// Забыть токен аккаунта `email` для `scope`: сервис его отверг (истёк, пока компьютер спал, или отозван).
pub fn forget(email: &str, scope: &str) {
    cache().remove(&(email.to_string(), scope.to_string()));
}

/// Токен доступа для области `scope`: из кэша, пока не истёк (минус минута), иначе новый обмен JWT.
pub async fn token(store: &Store, scope: &str) -> Result<String, String> {
    token_at(store, scope, &token_url()).await
}

#[cfg(not(test))]
fn token_url() -> String {
    TOKEN_URL.to_string()
}

#[cfg(test)]
fn token_url() -> String {
    tests::TOKEN_URL.with(|url| url.borrow().clone())
}

async fn token_at(store: &Store, scope: &str, url: &str) -> Result<String, String> {
    let json = stored(store)?.ok_or("Ключ Google не загружен. Добавьте его в разделе «Настройки → Интеграции».")?;
    let (key, pair) = parse(&json)?;
    let id = (key.client_email.clone(), scope.to_string());
    if let Some((token, until)) = cache().get(&id)
        && *until > SystemTime::now()
    {
        return Ok(token.clone());
    }
    let jwt = sign(&key, &pair, scope, SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs())?;
    let sent = services::http(store)?
        .post(url)
        .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", jwt.as_str())])
        .send()
        .await;
    let resp = sent.map_err(|e| format!("Google: токен не получен: {}", e.without_url()))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let text = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let access = text("access_token");
    if !status.is_success() || access.is_empty() {
        let reason: Vec<String> = [text("error"), text("error_description")].into_iter().filter(|t| !t.is_empty()).collect();
        return Err(format!("Google: токен не получен ({status}) {}", reason.join(" ")).trim_end().to_string());
    }
    let life = body.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    let until = SystemTime::now() + Duration::from_secs(life).saturating_sub(MARGIN);
    cache().insert(id, (access.clone(), until));
    Ok(access)
}

fn sign(key: &Key, pair: &RsaKeyPair, scope: &str, now: u64) -> Result<String, String> {
    let head = B64URL.encode(json!({ "alg": "RS256", "typ": "JWT" }).to_string());
    let claims = json!({ "iss": key.client_email, "scope": scope, "aud": key.token_uri, "iat": now, "exp": now + JWT_TTL });
    let signing = format!("{head}.{}", B64URL.encode(claims.to_string()));
    let mut sig = vec![0u8; pair.public().modulus_len()];
    pair.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), signing.as_bytes(), &mut sig)
        .map_err(|_| "Google: не удалось подписать запрос ключом".to_string())?;
    Ok(format!("{signing}.{}", B64URL.encode(sig)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// Куда ходит `token()` в тестах других модулей (подставной endpoint токена).
        pub(crate) static TOKEN_URL: RefCell<String> = RefCell::new("http://127.0.0.1:9/token".into());
    }
    use crate::store::secrets::SecretBox;
    use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) fn store() -> Store {
        Store::open_in_memory(SecretBox::from_key(&[3u8; 32])).unwrap()
    }

    /// Только для тестов: RSA-ключ создан для тестов и нигде не зарегистрирован — в сборку приложения не попадает.
    const TEST_PEM: &str = include_str!("google_test_key.pem");

    /// Ключ-файл с уникальным адресом: общий кэш процесса не смешивает параллельные тесты.
    pub(crate) fn key_json(email: &str, kind: &str) -> String {
        json!({ "type": kind, "client_email": email, "private_key": TEST_PEM, "token_uri": "https://oauth2.googleapis.com/token" }).to_string()
    }

    #[test]
    fn key_is_saved_encrypted_and_email_returned() {
        let s = store();
        assert_eq!(key_email(&s).unwrap(), None);
        assert_eq!(set_key(&s, &key_json("a@x.iam", "service_account")).unwrap(), "a@x.iam");
        assert_eq!(key_email(&s).unwrap().as_deref(), Some("a@x.iam"));
        let raw = s.get_setting(GOOGLE_KEY_KEY).unwrap().unwrap();
        assert!(raw.starts_with("enc:") && !raw.contains("PRIVATE KEY") && !raw.contains("a@x.iam"));
        clear_key(&s).unwrap();
        assert_eq!(key_email(&s).unwrap(), None);
        clear_key(&s).unwrap();
    }

    #[test]
    fn broken_key_is_not_saved() {
        let s = store();
        set_key(&s, &key_json("keep@x.iam", "service_account")).unwrap();
        for bad in [
            "не json".to_string(),
            key_json("b@x.iam", "authorized_user"),
            json!({ "type": "service_account", "client_email": "b@x.iam", "private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----", "token_uri": "u" }).to_string(),
            json!({ "type": "service_account", "client_email": "b@x.iam" }).to_string(),
        ] {
            let e = set_key(&s, &bad).unwrap_err();
            assert!(!e.contains("BEGIN"), "{e}");
        }
        assert_eq!(key_email(&s).unwrap().as_deref(), Some("keep@x.iam"));
    }

    /// Поддельный endpoint: считает обращения, запоминает тело, отдаёт токен со сроком `expires_in`.
    async fn fake(expires_in: u64) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let (h, b) = (hits.clone(), bodies.clone());
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move |body: String| {
                let n = h.fetch_add(1, Ordering::SeqCst) + 1;
                b.lock().unwrap().push(body);
                async move { axum::Json(json!({ "access_token": format!("at-{n}"), "expires_in": expires_in, "token_type": "Bearer" })) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/token", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, hits, bodies)
    }

    #[tokio::test]
    async fn jwt_is_signed_and_token_cached() {
        let s = store();
        set_key(&s, &key_json("cache@x.iam", "service_account")).unwrap();
        let (url, hits, bodies) = fake(3600).await;
        assert_eq!(token_at(&s, "scope-a", &url).await.unwrap(), "at-1");
        assert_eq!(token_at(&s, "scope-a", &url).await.unwrap(), "at-1");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(token_at(&s, "scope-b", &url).await.unwrap(), "at-2");

        let body = bodies.lock().unwrap()[0].clone();
        let form: HashMap<String, String> = reqwest::Url::parse(&format!("http://x/?{body}")).unwrap().query_pairs().into_owned().collect();
        assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer");
        let parts: Vec<&str> = form["assertion"].split('.').collect();
        let pair = pem_pair(TEST_PEM).unwrap();
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, pair.public().as_ref())
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &B64URL.decode(parts[2]).unwrap())
            .unwrap();
        let claims: Value = serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "cache@x.iam");
        assert_eq!(claims["scope"], "scope-a");
        assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
        assert_eq!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(), 3600);
    }

    #[tokio::test]
    async fn expired_token_is_refetched() {
        let s = store();
        set_key(&s, &key_json("expire@x.iam", "service_account")).unwrap();
        let (url, hits, _) = fake(30).await; // срок короче запаса в минуту — кэш не держит
        assert_eq!(token_at(&s, "sc", &url).await.unwrap(), "at-1");
        assert_eq!(token_at(&s, "sc", &url).await.unwrap(), "at-2");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// Токен в кэше на час вперёд по настенным часам: как будто получен раньше.
    pub(crate) fn plant(email: &str, scope: &str, token: &str) {
        cache().insert((email.to_string(), scope.to_string()), (token.to_string(), SystemTime::now() + Duration::from_secs(3600)));
    }

    #[tokio::test]
    async fn expiry_is_by_the_wall_clock() {
        // Монотонные часы macOS стоят, пока компьютер спит; срок по настенным часам после сна уже прошёл.
        let s = store();
        set_key(&s, &key_json("sleep@x.iam", "service_account")).unwrap();
        let (url, hits, _) = fake(3600).await;
        let past = SystemTime::now() - Duration::from_secs(1);
        cache().insert(("sleep@x.iam".to_string(), "sc".to_string()), ("old".to_string(), past));
        assert_eq!(token_at(&s, "sc", &url).await.unwrap(), "at-1");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refused_token_error_has_single_spaces() {
        let s = store();
        set_key(&s, &key_json("refused@x.iam", "service_account")).unwrap();
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(|| async { (axum::http::StatusCode::BAD_REQUEST, axum::Json(json!({ "error_description": "Invalid JWT Signature." }))) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/token", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let e = token_at(&s, "sc", &url).await.unwrap_err();
        assert_eq!(e, "Google: токен не получен (400 Bad Request) Invalid JWT Signature.");
    }

    #[tokio::test]
    async fn no_key_is_an_error() {
        let e = token_at(&store(), "sc", "http://127.0.0.1:9/token").await.unwrap_err();
        assert!(e.contains("Ключ Google не загружен"), "{e}");
    }
}
