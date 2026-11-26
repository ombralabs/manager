use super::*;
const NOW: i64 = 1_790_845_200;
const PASSWORD: &str = "a long example user password";

fn configured(dir: &std::path::Path) -> (Security, SetupDetails, String) {
    let mut security = Security::load(Paths::new(dir)).unwrap();
    security.test_device = Some([7; 32]);
    let details = security.begin_setup(PASSWORD, NOW).unwrap();
    let seed = details.secret.clone();
    let code = current_code(&seed, NOW);
    assert!(security.finish_setup(true, NOW).is_err(), "без проверенного кода нельзя включить защиту");
    security.verify_setup(&code, NOW).unwrap();
    assert!(security.finish_setup(false, NOW).is_err(), "нужно подтвердить сохранение восстановления");
    let mut pending = security.pending.take().unwrap();
    pending.config.migrating = false;
    pending.config.wrapped = wrap(&combined(&pending.password_key, &*pending.device), &pending.payload).unwrap();
    security.save(&pending.config).unwrap();
    security.config = Some(pending.config);
    (security, details, seed)
}
fn current_code(seed: &str, now: i64) -> String { crate::app::totp::code_at(&crate::app::totp::decode_base32(seed).unwrap(), now).0 }

#[test]
fn device_file_survives_restart_and_recovery_replaces_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let mut security = Security::load(paths.clone()).unwrap();
    let details = security.begin_setup(PASSWORD, NOW).unwrap();
    assert!(paths.device_key_file().exists());
    security.verify_setup(&current_code(&details.secret, NOW), NOW).unwrap();
    security.finish_setup(true, NOW).unwrap();
    drop(security);
    let mut restarted = Security::load(paths.clone()).unwrap();
    assert!(restarted.authenticate(PASSWORD, &current_code(&details.secret, NOW + 30), false, NOW + 30).is_ok());
    fs::remove_file(paths.device_key_file()).unwrap();
    assert!(restarted.authenticate(PASSWORD, &details.recovery_codes[0], true, NOW + 60).is_ok());
    assert!(paths.device_key_file().exists());
    drop(restarted);
    let mut recovered = Security::load(paths).unwrap();
    assert!(recovered.authenticate(PASSWORD, &current_code(&details.secret, NOW + 90), false, NOW + 90).is_ok());
}

#[test]
fn without_2fa_native_core_and_noninteractive_cli_remain_available() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let mut security = Security::load(paths.clone()).unwrap();
    assert!(!security.status(NOW).configured);
    assert!(!security.status(NOW).unlocked);
    assert!(security.unlock(PASSWORD, "123456", false, NOW).is_err());
    let core = crate::app::open_default(paths.clone(), std::sync::Arc::new(|_| {})).unwrap();
    core.store.set_setting("ordinary-session", "available").unwrap();
    assert!(paths.db().exists());
    assert_eq!(core.store.get_setting("ordinary-session").unwrap().as_deref(), Some("available"));
    let (cli_paths, instance) = unlock_terminal(paths.clone()).unwrap();
    assert!(!cli_paths.is_protected() && instance.is_none());
    assert_eq!(crate::app::open_default(paths.in_vault(), std::sync::Arc::new(|_| {})).err().unwrap().code, "SecurityLocked");
    append_log(&paths, b"ordinary session log").unwrap();
    append_log(&paths.in_vault(), b"must not be saved before mount").unwrap();
    assert!(paths.logs().join("ombra.log").exists() && !paths.in_vault().logs().exists());
    assert!(!paths.in_vault().profile_dir("test").starts_with(paths.root()), "нельзя вынести кэш Chromium из зашифрованного тома");
}

#[test]
fn enabling_2fa_stages_migration_for_restart_and_prohibits_plaintext_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let core = crate::app::open_default(paths.clone(), std::sync::Arc::new(|_| {})).unwrap();
    core.store.set_setting("before-protection", "preserved").unwrap();
    let mut security = Security::load(paths.clone()).unwrap();
    security.test_device = Some([7; 32]);
    let details = security.begin_setup(PASSWORD, NOW).unwrap();
    // До завершения настройки обычный сеанс продолжает создавать данные.
    fs::create_dir_all(paths.profile_dir("created-during-setup")).unwrap();
    security.verify_setup(&current_code(&details.secret, NOW), NOW).unwrap();
    assert!(security.finish_setup(true, NOW).unwrap().is_none(), "перенос ждёт завершения открытого ядра");
    assert!(security.config.as_ref().unwrap().items.contains(&"profiles".into()));
    assert!(security.status(NOW).configured && security.status(NOW).migrating && !security.status(NOW).unlocked);
    assert!(!paths.vault_image().exists());
    assert!(paths.db().exists(), "открытая база не переносится и не удаляется до перезапуска");
    assert_eq!(core.store.get_setting("before-protection").unwrap().as_deref(), Some("preserved"));
    assert_eq!(crate::app::open_default(paths.clone(), std::sync::Arc::new(|_| {})).err().unwrap().code, "SecurityLocked");
    append_log(&paths, b"must not leak after activation").unwrap();
    assert!(!paths.logs().exists());
}

#[test]
fn settings_never_contain_plaintext_password_seed_or_recovery_codes() {
    let dir = tempfile::tempdir().unwrap();
    let (security, details, _) = configured(dir.path());
    let text = fs::read_to_string(security.paths.security_config()).unwrap();
    for secret in std::iter::once(PASSWORD).chain(std::iter::once(details.secret.as_str())).chain(details.recovery_codes.iter().map(String::as_str)) {
        assert!(!text.contains(secret));
    }
    assert_eq!(security.config.as_ref().unwrap().recovery.len(), 8);
}

#[test]
fn both_factors_are_required_and_totp_is_not_replayed_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, _, seed) = configured(dir.path());
    assert!(security.authenticate(PASSWORD, &current_code(&seed, NOW), false, NOW).is_err(), "код настройки уже использован");
    let now = NOW + 30;
    let code = current_code(&seed, now);
    assert!(security.authenticate("incorrect password", &code, false, now).is_err());
    assert!(security.authenticate(PASSWORD, "abcdef", false, now).is_err());
    security.authenticate(PASSWORD, &code, false, now).unwrap();
    let mut restarted = Security::load(security.paths.clone()).unwrap();
    restarted.test_device = Some([7; 32]);
    assert!(!restarted.status(now).unlocked);
    assert!(restarted.authenticate(PASSWORD, &code, false, now).is_err());
    restarted.authenticate(PASSWORD, &current_code(&seed, now + 30), false, now + 30).unwrap();
}

#[test]
fn recovery_requires_password_is_single_use_and_repairs_a_new_device() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, details, seed) = configured(dir.path());
    security.test_device = Some([9; 32]);
    assert!(security.authenticate(PASSWORD, &current_code(&seed, NOW + 30), false, NOW + 30).is_err());
    assert!(security.authenticate("wrong password", &details.recovery_codes[0], true, NOW + 30).is_err());
    security.authenticate(PASSWORD, &details.recovery_codes[0], true, NOW + 30).unwrap();
    assert_eq!(security.status(NOW).recovery_remaining, 7);
    let mut restarted = Security::load(security.paths.clone()).unwrap();
    restarted.test_device = Some([9; 32]);
    assert!(restarted.authenticate(PASSWORD, &details.recovery_codes[0], true, NOW + 60).is_err());
    restarted.authenticate(PASSWORD, &current_code(&seed, NOW + 60), false, NOW + 60).unwrap();
}

#[test]
fn attempt_limit_survives_restart_and_blocks_valid_credentials_until_delay_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, _, seed) = configured(dir.path());
    for _ in 0..5 { assert!(security.authenticate(PASSWORD, "wrong", false, NOW).is_err()); }
    let mut restarted = Security::load(security.paths.clone()).unwrap();
    restarted.test_device = Some([7; 32]);
    assert_eq!(restarted.status(NOW).retry_after, 30);
    assert_eq!(restarted.authenticate(PASSWORD, &current_code(&seed, NOW), false, NOW).err().unwrap().code, "SecurityRateLimited");
    restarted.authenticate(PASSWORD, &current_code(&seed, NOW + 30), false, NOW + 30).unwrap();
    assert_eq!(restarted.status(NOW + 30).retry_after, 0);
}

#[test]
fn password_change_requires_reauthentication_and_revokes_previous_recovery_codes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, details, seed) = configured(dir.path());
    security.unlocked = true;
    let code = current_code(&seed, NOW + 30);
    assert!(security.change_password("wrong password", &code, "a different long password", NOW + 30).is_err());
    let fresh = security.change_password(PASSWORD, &code, "a different long password", NOW + 30).unwrap();
    assert_eq!(fresh.len(), 8);
    assert!(security.authenticate(PASSWORD, &fresh[0], true, NOW + 60).is_err());
    assert!(security.authenticate("a different long password", &details.recovery_codes[0], true, NOW + 60).is_err());
    security.authenticate("a different long password", &fresh[0], true, NOW + 60).unwrap();
}

#[test]
fn changing_authenticator_is_staged_and_cancelling_preserves_the_existing_factor() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, _, seed) = configured(dir.path());
    security.unlocked = true;
    let next = security.replace_authenticator(PASSWORD, &current_code(&seed, NOW + 30), NOW + 30).unwrap();
    assert_ne!(next.secret, seed);
    security.cancel_setup();
    security.authenticate(PASSWORD, &current_code(&seed, NOW + 60), false, NOW + 60).unwrap();
    let next = security.replace_authenticator(PASSWORD, &current_code(&seed, NOW + 90), NOW + 90).unwrap();
    security.verify_setup(&current_code(&next.secret, NOW + 90), NOW + 90).unwrap();
    security.finish_setup(true, NOW + 90).unwrap();
    assert!(security.authenticate(PASSWORD, &current_code(&seed, NOW + 120), false, NOW + 120).is_err());
    security.authenticate(PASSWORD, &current_code(&next.secret, NOW + 120), false, NOW + 120).unwrap();
}

#[test]
fn incomplete_or_corrupt_protection_never_falls_back_to_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    fs::create_dir(paths.vault_image()).unwrap();
    assert!(Security::load(paths.clone()).is_err());
    fs::write(paths.security_config(), "{}").unwrap();
    assert!(Security::load(paths).is_err());
}

#[test]
fn a_recent_recovery_unlock_can_replace_a_lost_authenticator_even_after_the_last_code() {
    let dir = tempfile::tempdir().unwrap();
    let (mut security, _, _) = configured(dir.path());
    security.unlocked = true;
    security.config.as_mut().unwrap().recovery.clear();
    security.recovery_verified_at = Some(NOW);
    assert!(security.replace_authenticator("wrong password", "", NOW + 1).is_err());
    let details = security.replace_authenticator(PASSWORD, "", NOW + 1).unwrap();
    security.verify_setup(&current_code(&details.secret, NOW + 1), NOW + 1).unwrap();
    security.finish_setup(true, NOW + 1).unwrap();
    assert!(!security.status(NOW + 1).recovery_recent);
    assert_eq!(security.status(NOW + 1).recovery_remaining, 8);
    assert!(security.replace_authenticator(PASSWORD, "", NOW + 2).is_err(), "разрешение восстановления расходуется после замены");
    security.recovery_verified_at = Some(NOW);
    assert!(security.replace_authenticator(PASSWORD, "", NOW + 301).is_err(), "недавний вход не даёт бессрочного обхода повторной проверки");
}
