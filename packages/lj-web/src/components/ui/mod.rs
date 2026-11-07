//! Kit UI partage. Composants partages : NE PAS dupliquer cote tranches.

pub mod badge;
pub mod button;
pub mod card;
pub mod date_picker;
pub mod dropdown_select;
pub mod expandable_summary;
pub mod grouped_select;
pub mod input;
pub mod panel;
pub mod pill;
pub mod provenance;
pub mod separator;
pub mod skeleton;
pub mod summary;

pub use badge::{badge_classes, Badge, BadgeTone};
pub use button::{button_classes, Button, ButtonSize, ButtonVariant};
pub use card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle};
pub use date_picker::{DatePicker, DateRangePicker};
pub use dropdown_select::DropdownSelect;
pub use expandable_summary::ExpandableSummary;
pub use grouped_select::{GroupedOption, GroupedSelect, SelectGroup};
pub use input::Input;
pub use pill::Pill;
pub use provenance::ProvenanceBlock;
pub use separator::Separator;
pub use skeleton::Skeleton;
pub use summary::{shown_summary, SummaryBadge, SummaryFold};

/// Option `{value, label}` partagee par les selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}
