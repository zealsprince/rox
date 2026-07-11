//! The main window: an in-window menubar over a placeholder content area.
//! GPUI only surfaces `set_menus` in the macOS system bar, so the bar is
//! drawn in-window to behave the same on every platform.

use gpui::{deferred, div, prelude::*, px, rgb, Context, MouseButton, Window};

const MENU_BAR_H: f32 = 30.0;

#[derive(Clone, Copy)]
enum MenuAction {
    NewWindow,
    OpenViz,
    OpenPlayback,
}

struct MenuItem {
    label: &'static str,
    action: MenuAction,
}

struct Menu {
    label: &'static str,
    items: &'static [MenuItem],
}

const MENUS: &[Menu] = &[
    Menu {
        label: "Window",
        items: &[MenuItem {
            label: "New Window",
            action: MenuAction::NewWindow,
        }],
    },
    Menu {
        label: "Prototypes",
        items: &[
            MenuItem {
                label: "Visualizer",
                action: MenuAction::OpenViz,
            },
            MenuItem {
                label: "Playback",
                action: MenuAction::OpenPlayback,
            },
        ],
    },
];

pub struct Workspace {
    open_menu: Option<usize>,
}

impl Workspace {
    pub fn new() -> Self {
        Workspace { open_menu: None }
    }

    fn run(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        match action {
            MenuAction::NewWindow => crate::open_workspace(cx),
            MenuAction::OpenViz => rox_prototype_viz::open_window(cx),
            MenuAction::OpenPlayback => crate::playback::open_window(cx),
        }
    }

    fn menu_button(&self, index: usize, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_menu == Some(index);
        div()
            .relative()
            .h_full()
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(0x333333)))
            .hover(|d| d.bg(rgb(0x2f2f2f)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = if this.open_menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                    cx.notify();
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open_menu = None;
                    cx.notify();
                }))
            })
            .child(menu.label)
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }

    fn dropdown(&self, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .top(px(MENU_BAR_H))
            .min_w(px(180.))
            .flex()
            .flex_col()
            .py_1()
            .bg(rgb(0x262626))
            .border_1()
            .border_color(rgb(0x3a3a3a))
            .shadow_md()
            .occlude()
            .children(menu.items.iter().map(|item| {
                let action = item.action;
                div()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(0x3a3a3a)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_menu = None;
                            cx.notify();
                            this.run(action, cx);
                        }),
                    )
                    .child(item.label)
            }))
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(MENU_BAR_H))
                    .flex_none()
                    .bg(rgb(0x242424))
                    .border_b_1()
                    .border_color(rgb(0x333333))
                    .children(
                        MENUS
                            .iter()
                            .enumerate()
                            .map(|(i, menu)| self.menu_button(i, menu, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_xl().child("rox"))
                    .child(
                        div()
                            .text_color(rgb(0x808080))
                            .child("If Foobar2000 was made this year."),
                    ),
            )
    }
}
