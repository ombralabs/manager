//! Меню приложения со своим пунктом «Выйти»: Cmd+Q должен спросить про запущенные профили.
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

pub const QUIT: &str = "quit";

pub fn install<R: Runtime>(app: &AppHandle<R>, language: &str) -> tauri::Result<()> {
    let (quit_label, edit_label) = if language == "en" {
        ("Quit Ombra", "Edit")
    } else {
        ("Выйти из Ombra", "Правка")
    };
    let quit = MenuItemBuilder::with_id(QUIT, quit_label).accelerator("CmdOrCtrl+Q").build(app)?;
    let app_menu = SubmenuBuilder::new(app, "Ombra").about(None);
    #[cfg(target_os = "macos")]
    let app_menu = app_menu
        .separator()
        .hide()
        .hide_others()
        .show_all();
    let app_menu = app_menu.separator().item(&quit).build()?;
    // Без меню «Правка» в полях ввода не работают Cmd+C и Cmd+V.
    let edit_menu = SubmenuBuilder::new(app, edit_label);
    #[cfg(not(target_os = "linux"))]
    let edit_menu = edit_menu
        .undo()
        .redo()
        .separator();
    let edit_menu = edit_menu
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let menu = MenuBuilder::new(app).items(&[&app_menu, &edit_menu]);
    // GTK does not implement the predefined minimize/close menu items.
    #[cfg(not(target_os = "linux"))]
    let menu = {
        let window_label = if language == "en" { "Window" } else { "Окно" };
        let window_menu = SubmenuBuilder::new(app, window_label).minimize().close_window().build()?;
        menu.item(&window_menu)
    };
    app.set_menu(menu.build()?)?;
    Ok(())
}
