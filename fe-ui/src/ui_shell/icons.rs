//! Font-independent semantic button icons; see ui_shell/AGENTS.md.

use bevy_egui::egui;
use crate::settings::TreeIcon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Icon {
    Select,
    Move,
    Rotate,
    Scale,
    Pen,
    Brush,
    Measure,
    Close,
    Info,
    Options,
    Terrain,
    Report,
    Settings,
    Maps,
    Groups,
    Data,
    Expand,
    Collapse,
    Visible,
    Hidden,
    Mixed,
}

impl Icon {
    pub(crate) fn visibility(visible: bool) -> Self {
        if visible {
            Self::Visible
        } else {
            Self::Hidden
        }
    }

    pub(crate) fn visibility_rollup(state: crate::visibility::RollupState) -> Self {
        use crate::visibility::RollupState;
        match state {
            RollupState::AllVisible => Self::Visible,
            RollupState::AllHidden => Self::Hidden,
            RollupState::Mixed => Self::Mixed,
        }
    }

    pub(crate) fn button(self, ui: &egui::Ui) -> IconButton {
        IconButton::new(ui, self, None)
    }

    pub(crate) fn text_button(
        self,
        ui: &egui::Ui,
        text: impl Into<egui::WidgetText>,
    ) -> IconButton {
        IconButton::new(ui, self, Some(text.into()))
    }

    fn paint(self, painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let side = rect.width().min(rect.height()).min(14.0);
        let rect = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(side));
        let inner = rect.shrink(1.25);
        let point = |x: f32, y: f32| egui::pos2(inner.left() + inner.width() * x, inner.top() + inner.height() * y);
        let stroke = egui::Stroke::new(1.25, color);
        let line = |points: &[[f32; 2]]| {
            for pair in points.windows(2) {
                painter.line_segment([point(pair[0][0], pair[0][1]), point(pair[1][0], pair[1][1])], stroke);
            }
        };
        let circle = |x: f32, y: f32, radius: f32| {
            painter.circle_stroke(point(x, y), radius * inner.width(), stroke);
        };
        match self {
            Self::Select => TreeIcon::Square.paint(painter, rect, color),
            Self::Visible | Self::Brush => TreeIcon::Circle.paint(painter, rect, color),
            Self::Hidden => TreeIcon::Ring.paint(painter, rect, color),
            Self::Close => {
                line(&[[0.1, 0.1], [0.9, 0.9]]);
                line(&[[0.9, 0.1], [0.1, 0.9]]);
            }
            Self::Move => {
                line(&[[0.5, 0.0], [0.5, 1.0]]);
                line(&[[0.0, 0.5], [1.0, 0.5]]);
                for (tip, a, b) in [([0.5, 0.0], [0.3, 0.2], [0.7, 0.2]),
                    ([0.5, 1.0], [0.3, 0.8], [0.7, 0.8]),
                    ([0.0, 0.5], [0.2, 0.3], [0.2, 0.7]),
                    ([1.0, 0.5], [0.8, 0.3], [0.8, 0.7])] {
                    line(&[a, tip, b]);
                }
            }
            Self::Rotate => {
                for i in 0..12 {
                    let a = i as f32 * std::f32::consts::TAU / 16.0;
                    let b = (i + 1) as f32 * std::f32::consts::TAU / 16.0;
                    line(&[[0.5 + 0.4 * a.cos(), 0.5 + 0.4 * a.sin()],
                        [0.5 + 0.4 * b.cos(), 0.5 + 0.4 * b.sin()]]);
                }
                line(&[[0.3, 0.05], [0.5, 0.1], [0.4, 0.3]]);
            }
            Self::Scale => {
                line(&[[0.05, 0.95], [0.95, 0.05]]);
                line(&[[0.05, 0.55], [0.05, 0.95], [0.45, 0.95]]);
                line(&[[0.55, 0.05], [0.95, 0.05], [0.95, 0.45]]);
            }
            Self::Pen => line(&[[0.1, 0.9], [0.2, 0.6], [0.75, 0.05], [0.95, 0.25], [0.4, 0.8], [0.1, 0.9]]),
            Self::Measure => {
                line(&[[0.0, 0.5], [1.0, 0.5]]);
                line(&[[0.2, 0.3], [0.0, 0.5], [0.2, 0.7]]);
                line(&[[0.8, 0.3], [1.0, 0.5], [0.8, 0.7]]);
            }
            Self::Info => {
                TreeIcon::Ring.paint(painter, rect, color);
                line(&[[0.5, 0.45], [0.5, 0.75]]);
                painter.circle_filled(point(0.5, 0.25), 0.7, color);
            }
            Self::Options => {
                line(&[[0.7, 0.0], [0.55, 0.2], [0.65, 0.4], [0.85, 0.45], [1.0, 0.3]]);
                line(&[[0.95, 0.5], [0.2, 1.0], [0.0, 0.8], [0.5, 0.25]]);
            }
            Self::Terrain => {
                line(&[[0.0, 0.9], [0.4, 0.1], [0.65, 0.6], [0.8, 0.35], [1.0, 0.9], [0.0, 0.9]]);
                line(&[[0.25, 0.4], [0.4, 0.5], [0.5, 0.3]]);
            }
            Self::Report => {
                line(&[[0.15, 0.0], [0.65, 0.0], [0.9, 0.25], [0.9, 1.0], [0.15, 1.0], [0.15, 0.0]]);
                line(&[[0.65, 0.0], [0.65, 0.25], [0.9, 0.25]]);
                line(&[[0.3, 0.5], [0.75, 0.5]]);
                line(&[[0.3, 0.75], [0.75, 0.75]]);
            }
            Self::Settings => {
                circle(0.5, 0.5, 0.3);
                circle(0.5, 0.5, 0.1);
                for i in 0..8 {
                    let angle = i as f32 * std::f32::consts::TAU / 8.0;
                    line(&[[0.5 + 0.3 * angle.cos(), 0.5 + 0.3 * angle.sin()],
                        [0.5 + 0.48 * angle.cos(), 0.5 + 0.48 * angle.sin()]]);
                }
            }
            Self::Maps => {
                line(&[[0.5, 0.0], [1.0, 0.25], [1.0, 0.75], [0.5, 1.0], [0.0, 0.75], [0.0, 0.25], [0.5, 0.0]]);
                line(&[[0.0, 0.25], [0.5, 0.5], [1.0, 0.25]]);
                line(&[[0.5, 0.5], [0.5, 1.0]]);
            }
            Self::Groups => {
                circle(0.5, 0.25, 0.2);
                circle(0.2, 0.75, 0.2);
                circle(0.8, 0.75, 0.2);
            }
            Self::Data => {
                line(&[[0.0, 0.15], [0.33, 0.0], [0.67, 0.15], [1.0, 0.0], [1.0, 0.85], [0.67, 1.0], [0.33, 0.85], [0.0, 1.0], [0.0, 0.15]]);
                line(&[[0.33, 0.0], [0.33, 0.85]]);
                line(&[[0.67, 0.15], [0.67, 1.0]]);
            }
            Self::Expand | Self::Collapse => {
                let points = if self == Self::Expand {
                    vec![point(0.3, 0.1), point(0.8, 0.5), point(0.3, 0.9)]
                } else { vec![point(0.1, 0.3), point(0.9, 0.3), point(0.5, 0.8)] };
                painter.add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
            }
            Self::Mixed => {
                TreeIcon::Ring.paint(painter, rect, color);
                let mut points = vec![point(0.5, 0.5)];
                for i in 0..=8 {
                    let angle = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 8.0;
                    points.push(point(0.5 + 0.4 * angle.cos(), 0.5 + 0.4 * angle.sin()));
                }
                painter.add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
            }
        }
    }
}

/// Adapter over the native Button custom-atom seam; see ui_shell/AGENTS.md.
pub(crate) struct IconButton {
    button: egui::Button<'static>,
    icon: Icon,
    color: Option<egui::Color32>,
}

fn icon_slot() -> egui::Id {
    egui::Id::new("fe-semantic-icon")
}

impl IconButton {
    fn new(ui: &egui::Ui, icon: Icon, text: Option<egui::WidgetText>) -> Self {
        let font = egui::FontSelection::Default
            .resolve_with_fallback(ui.style(), egui::TextStyle::Button.into());
        let size = font.size.clamp(2.0, 14.0);
        let mut atoms = egui::Atoms::new(egui::Atom::custom(icon_slot(), egui::Vec2::splat(size)));
        if let Some(text) = text {
            atoms.push_right(text);
        }
        Self {
            button: egui::Button::new(atoms)
                .min_size(egui::Vec2::splat(super::layout_policy::target_size(ui.ctx()))),
            icon,
            color: None,
        }
    }
}

impl IconButton {
    pub(crate) fn fill(mut self, color: egui::Color32) -> Self {
        self.button = self.button.fill(color);
        self
    }

    pub(crate) fn frame(mut self, frame: bool) -> Self {
        self.button = self.button.frame(frame);
        self
    }

    pub(crate) fn min_size(mut self, size: egui::Vec2) -> Self {
        self.button = self.button.min_size(size);
        self
    }

    pub(crate) fn truncate(mut self) -> Self {
        self.button = self.button.truncate();
        self
    }

    pub(crate) fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }
}

impl egui::Widget for IconButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let painted = self.button.atom_ui(ui);
        if let Some(rect) = painted.rect(icon_slot()) {
            let color = self.color
                .unwrap_or_else(|| ui.style().interact(&painted.response).text_color());
            let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(painted.response.rect));
            self.icon.paint(&painter, rect, color);
        }
        painted.response
    }
}

#[cfg(test)]
pub(super) fn assert_icon_button_journey() {
    use crate::accessibility::ResponseExt;
    let icons = [Icon::Select, Icon::Move, Icon::Rotate, Icon::Scale, Icon::Pen,
        Icon::Brush, Icon::Measure, Icon::Close, Icon::Info, Icon::Options,
        Icon::Terrain, Icon::Report, Icon::Settings, Icon::Maps, Icon::Groups,
        Icon::Data, Icon::Expand, Icon::Collapse, Icon::Visible, Icon::Hidden, Icon::Mixed];
    for scale in [0.75, 1.0, 2.0] {
        for enabled in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            ctx.enable_accesskit();
            let draw = |events| {
                let mut responses = Vec::new();
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 120.0))),
                    events,
                    ..Default::default()
                }, |ctx| {
                    super::layout_policy::apply_style(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            for (index, icon) in icons.iter().copied().enumerate() {
                                responses.push(ui.add_enabled(enabled, icon.button(ui).frame(false))
                                    .named(ui, format!("Icon {index}")));
                            }
                        });
                    });
                });
                (output, responses)
            };
            let (output, responses) = draw(Vec::new());
            let update = output.platform_output.accesskit_update.as_ref().expect("native button semantics");
            for (index, response) in responses.iter().enumerate() {
                assert_eq!(response.enabled(), enabled);
                assert!(response.rect.width() * scale >= 20.0 && response.rect.height() * scale >= 20.0);
                let semantic = update.nodes.iter().find(|(id, _)| id.0 == response.id.value())
                    .map(|(_, node)| node).expect("named native button");
                assert_eq!(semantic.label(), Some(format!("Icon {index}").as_str()));
                assert_eq!(semantic.role(), egui::accesskit::Role::Button);
                let painted: Vec<_> = output.shapes.iter().filter(|shape| shape.clip_rect == response.rect).collect();
                assert!(!painted.is_empty(), "every semantic icon emits vector shapes");
                let mut bounds = egui::Rect::NOTHING;
                for shape in painted {
                    assert!(!matches!(&shape.shape, egui::Shape::Text(_)), "icons do not depend on font coverage");
                    bounds |= shape.shape.visual_bounding_rect();
                }
                assert!(response.rect.contains_rect(bounds), "painted vectors fit the native control");
                assert!(bounds.width() <= 14.01 && bounds.height() <= 14.01, "icon paint stays text-sized");
            }
            let at = responses[0].rect.center();
            let pointer = |pressed| egui::Event::PointerButton {
                pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            };
            let _ = draw(vec![egui::Event::PointerMoved(at)]);
            let _ = draw(vec![pointer(true)]);
            let (_, released) = draw(vec![pointer(false)]);
            assert_eq!(released[0].id, responses[0].id, "custom atoms add no competing response identity");
            assert_eq!(released[0].clicked(), enabled, "native disabled/click gating is retained");
        }
    }
}
