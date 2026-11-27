#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod backup;
mod curl;
mod history;
mod http;
mod oauth;
mod postman;
mod prepare;
mod workbench;
mod workspace;
mod workspace_edit;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::Root;

struct Desktop {
    workbench: Entity<workbench::Workbench>,
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.workbench.clone())
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (path, writable) = match args.as_slice() {
        [] => {
            let original = workspace::default_workspace_path();
            let owned = original.as_deref().and_then(|path| {
                Some(
                    path.parent()?
                        .parent()?
                        .join("app.postlite.gpui")
                        .join("workspace.json"),
                )
            });
            let source = owned
                .as_ref()
                .filter(|path| path.is_file())
                .cloned()
                .or_else(|| original.filter(|path| path.is_file()));
            (source, owned)
        }
        [flag, path] if flag == "--workspace" => (Some(PathBuf::from(path)), None),
        [flag, path] if flag == "--write-workspace" => {
            let path = PathBuf::from(path);
            (path.exists().then(|| path.clone()), Some(path))
        }
        _ => {
            eprintln!("Usage: postlite-gpui [--workspace PATH | --write-workspace PATH]");
            std::process::exit(2);
        }
    };
    let loaded = path.as_deref().map(workspace::load).transpose();
    Application::new()
        .with_assets(appearance::Icons)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            appearance::load_fonts(cx);
            let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
            cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Postlite GPUI".into()),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1080.), px(600.))),
                    // Matches the installed desktop entry so launchers show its icon.
                    app_id: Some("app.postlite.gpui".into()),
                    ..Default::default()
                },
                move |window, cx| {
                    let (document, error, writable) = match loaded {
                        Ok(document) => (document.unwrap_or_default(), None, writable),
                        Err(error) => (workspace::Document::default(), Some(error), None),
                    };
                    let workbench = cx
                        .new(|cx| workbench::Workbench::new(document, error, writable, window, cx));
                    let closing = workbench.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        closing
                            .update(cx, |view, cx| view.prepare_to_close(window, cx))
                            .unwrap_or(true)
                    });
                    let desktop = cx.new(|_| Desktop { workbench });
                    cx.new(|cx| Root::new(desktop, window, cx))
                },
            )
            .expect("could not open the Postlite window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
