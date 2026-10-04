//! An element that sets the rem size for everything inside it: the viewer's
//! own zoom level. Built on gpui's `Window::with_rem_size`, the way Zed sizes
//! its editor apart from its UI; the code is ours.

use gpui_kit::{
    AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, StyleRefinement, Styled, Window, div,
};

/// A `div` whose content measures rems against `rem_size` instead of the
/// window's rem size.
pub struct RemScope {
    div: Div,
    rem_size: Pixels,
}

impl RemScope {
    pub fn new(rem_size: Pixels) -> Self {
        Self {
            div: div(),
            rem_size,
        }
    }
}

impl Styled for RemScope {
    fn style(&mut self) -> &mut StyleRefinement {
        self.div.style()
    }
}

impl ParentElement for RemScope {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.div.extend(elements);
    }
}

impl IntoElement for RemScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RemScope {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.div.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.div.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let div = &mut self.div;
        window.with_rem_size(Some(self.rem_size), |window| {
            div.request_layout(id, inspector_id, window, cx)
        })
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let div = &mut self.div;
        window.with_rem_size(Some(self.rem_size), |window| {
            div.prepaint(id, inspector_id, bounds, request_layout, window, cx)
        })
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let div = &mut self.div;
        window.with_rem_size(Some(self.rem_size), |window| {
            div.paint(
                id,
                inspector_id,
                bounds,
                request_layout,
                prepaint,
                window,
                cx,
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Context, InteractiveElement, Render, TestAppContext, VisualTestContext, px, rems,
    };

    struct Scoped;

    impl Render for Scoped {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().debug_selector(|| "outside".into()).h(rems(1.0)))
                .child(
                    RemScope::new(px(32.0))
                        .child(div().debug_selector(|| "inside".into()).h(rems(1.0))),
                )
        }
    }

    #[gpui_kit::test]
    fn rems_inside_measure_against_its_size(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Scoped);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        let mut height = |selector| cx.debug_bounds(selector).unwrap().size.height;
        assert_eq!(height("inside"), px(32.0));
        assert_eq!(height("outside"), px(16.0), "the window's rem size");
    }
}
