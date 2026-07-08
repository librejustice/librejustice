//! `Separator` (port de `ui/separator.tsx`). `role=separator` + orientation.

use leptos::prelude::*;

use crate::helpers::cn;

/// Orientation. Defaut : `Horizontal`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SeparatorOrientation {
    #[default]
    Horizontal,
    Vertical,
}

impl SeparatorOrientation {
    fn aria(self) -> &'static str {
        match self {
            SeparatorOrientation::Horizontal => "horizontal",
            SeparatorOrientation::Vertical => "vertical",
        }
    }

    fn size(self) -> &'static str {
        match self {
            SeparatorOrientation::Horizontal => "h-px w-full",
            SeparatorOrientation::Vertical => "h-full w-px",
        }
    }
}

/// Separateur (regle visuelle). `role` defaut `separator`.
#[component]
pub fn Separator(
    #[prop(optional, into)] orientation: Signal<SeparatorOrientation>,
    #[prop(default = "separator", into)] role: &'static str,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let o = orientation.get_untracked();
    let classes = cn(["bg-[var(--color-rule)]", o.size(), &class]);
    view! { <div role=role aria-orientation=o.aria() class=classes></div> }
}
