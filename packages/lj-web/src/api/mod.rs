//! Client API. Marqueur de la separation front(:3000) / API : ce module est le
//! SEUL endroit qui connait l'URL de l'API.
pub mod client;

pub use client::{ApiClient, ApiError, PageParams, TextesFilters};
