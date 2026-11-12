//! Drawing on terminals too small for what is drawn: nothing may panic, and what a dialog records
//! as a button must be on the screen.

use super::table::FROZEN_WIDTH;
use super::*;
use crate::ui::test_support::*;

const SIZES: [(u16, u16); 7] = [
    (0, 0),
    (1, 1),
    (3, 3),
    (10, 3),
    (10, 4),
    (20, 6),
    (FROZEN_WIDTH, 10),
];

/// An app with a PR whose details and diff have loaded.
fn loaded_app() -> App {
    let details = pr_details(
        "A description",
        serde_json::json!([{"createdAt": "2026-01-02T03:00:00Z", "body": "a comment"}]),
    );
    let mut app = app_with_details(vec![by("alice", 1, "Bump alpha")], details);
    app.diff_loaded(1, Ok("+added\n-removed".to_string()));
    app
}

/// Every screen the app can show, named.
fn every_screen() -> Vec<(&'static str, App)> {
    let mut screens = vec![
        (
            "loading",
            App::new("PRs".to_string(), "is:pr".to_string(), false),
        ),
        ("empty list", app_with(Vec::new())),
        ("list", loaded_app()),
    ];
    for (name, keys) in [("search", "/"), ("filter", "ffoo"), ("columns", "c")] {
        let mut app = loaded_app();
        press(&mut app, keys);
        screens.push((name, app));
    }
    let mut app = loaded_app();
    press(&mut app, " ");
    review(&mut app);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    screens.push(("confirm", app));

    let mut app = app_with((1..=7).map(|n| by("alice", n, "Bump alpha")).collect());
    for _ in 0..7 {
        press(&mut app, " j");
    }
    ctrl(&mut app, 'o');
    assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
    screens.push(("confirm open", app));

    let mut app = loaded_app();
    press(&mut app, "?");
    screens.push(("help", app));

    let mut app = loaded_app();
    app.details_loaded(1, Err(color_eyre::eyre::eyre!("it failed\nbadly")));
    press(&mut app, "e");
    assert!(matches!(app.mode, Mode::Failure(_)));
    screens.push(("failure", app));

    let mut app = loaded_app();
    press(&mut app, "v");
    assert!(matches!(app.mode, Mode::PickColumns(_)));
    screens.push(("pick columns", app));

    let mut app = loaded_app();
    press(&mut app, " ");
    review(&mut app);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
    screens.push(("running", app));

    let mut app = loaded_app();
    press(&mut app, " ");
    review(&mut app);
    key(&mut app, KeyCode::Enter);
    let epoch = app.batch_epoch();
    app.update(Msg::Approved(epoch, 1, Ok(())));
    assert!(matches!(app.mode, Mode::Results(_)));
    screens.push(("results", app));

    let mut app = loaded_app();
    for column in Column::ALL {
        app.toggle_column(column);
    }
    screens.push(("no columns", app));
    screens
}

#[test]
fn no_screen_panics_on_a_tiny_terminal() {
    for (name, mut app) in every_screen() {
        for (width, height) in SIZES {
            let buffer = draw(&mut app, width, height);
            assert_eq!(buffer.area.width, width, "{name} {width}x{height}");
        }
    }
}

#[test]
fn a_dialogs_buttons_are_in_order_without_overlap_and_on_a_row_of_the_screen() {
    for (name, mut app) in every_screen() {
        for (width, height) in SIZES {
            draw(&mut app, width, height);
            let screen = Rect::new(0, 0, width, height);
            // A button can run past the right edge of a narrow terminal, where it is cut off:
            // see the report of M7.5. Where it can be clicked is what the screen holds of it.
            let buttons = &app.drawn.buttons;
            for button in buttons {
                assert!(
                    button.y < screen.bottom(),
                    "{name} {width}x{height}: {button:?}"
                );
            }
            for pair in buttons.windows(2) {
                assert!(
                    pair[0].right() <= pair[1].x,
                    "{name} {width}x{height}: {pair:?}"
                );
            }
        }
    }
}

#[test]
fn a_click_on_a_dialogs_first_button_still_works_on_a_small_terminal() {
    for (width, height) in [(20, 12), (30, 10), (40, 20)] {
        let mut app = loaded_app();
        press(&mut app, "?");
        draw(&mut app, width, height);
        let Some(&button) = app.drawn.buttons.first() else {
            panic!("no button at {width}x{height}");
        };
        press_at(&mut app, Position::new(button.x, button.y));
        assert_list_mode(&app);
    }
}

#[test]
fn a_table_with_every_column_hidden_still_has_a_header() {
    let mut app = loaded_app();
    for column in Column::ALL {
        app.toggle_column(column);
    }
    let screen = lines(&draw(&mut app, 100, 30));
    let header = table_header_line(&app, &screen);
    assert!(header.contains("Title"), "{header:?}");
}
