//! Kit UI partage (port de `apps/web/src/components/ui/`). Memes classes
//! Tailwind EXACTES et meme hierarchie DOM que les composants React. Les
//! variantes CVA sont portees en fns Rust renvoyant les memes chaines de
//! classes. Composants partages : NE PAS dupliquer cote tranches.

pub mod badge;
pub mod button;
pub mod card;
pub mod dropdown_select;
pub mod inline_select;
pub mod input;
pub mod pill;
pub mod select;
pub mod separator;
pub mod skeleton;

pub use badge::{badge_classes, Badge, BadgeTone};
pub use button::{button_classes, Button, ButtonSize, ButtonVariant};
pub use card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle};
pub use dropdown_select::DropdownSelect;
pub use inline_select::InlineSelect;
pub use input::Input;
pub use pill::Pill;
pub use select::Select;
pub use separator::Separator;
pub use skeleton::Skeleton;

/// Option `{value, label}` partagee par les selects (dropdown / inline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}
