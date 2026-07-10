//! First light: prove GPUI renders on this machine and that opening a second
//! OS window works, since Wayland pop-out is open question 1 in the spec.

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, MouseButton, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

struct FirstLight {
    label: SharedString,
    popouts: usize,
}

fn window_options(title: &str, cx: &mut App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(640.), px(400.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title.to_string())),
            ..Default::default()
        }),
        ..Default::default()
    }
}

impl Render for FirstLight {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .child(div().text_xl().child(self.label.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x808080))
                    .child("click anywhere to pop out a new OS window"),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.popouts += 1;
                    let label: SharedString = format!("pop-out {}", this.popouts).into();
                    let options = window_options(&label, cx);
                    cx.open_window(options, |_, cx| {
                        cx.new(|_| FirstLight { label, popouts: 0 })
                    })
                    .ok();
                }),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let options = window_options("rox", cx);
        cx.open_window(options, |_, cx| {
            cx.new(|_| FirstLight {
                label: SharedString::from("rox first light"),
                popouts: 0,
            })
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
