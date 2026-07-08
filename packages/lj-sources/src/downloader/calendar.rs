//! Helpers calendaires partagés par les deux syncs (port des fonctions de dates).

use chrono::{Datelike, NaiveDate};

/// `["YYYYMM", ...]` inclus de `earliest` au mois courant (port de `_month_range`).
pub(super) fn month_range(earliest: &str, today: NaiveDate) -> Vec<String> {
    let (mut y, mut m) = parse_year_month(earliest);
    let (end_y, end_m) = (today.year(), today.month() as i32);
    let mut out = Vec::new();
    while (y, m) <= (end_y, end_m) {
        out.push(format!("{y:04}{m:02}"));
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    out
}

/// Parse `"YYYY-MM"` → `(year, month)`.
fn parse_year_month(s: &str) -> (i32, i32) {
    let y: i32 = s[0..4].parse().unwrap_or(1970);
    let m: i32 = s[5..7].parse().unwrap_or(1);
    (y, m)
}

/// `(date_start, date_end)` ISO inclusifs d'un mois `YYYYMM` (port de `_month_bounds`).
pub(super) fn month_bounds(yyyymm: &str) -> (String, String) {
    let year: i32 = yyyymm[0..4].parse().unwrap();
    let month: u32 = yyyymm[4..6].parse().unwrap();
    let start = format!("{year:04}-{month:02}-01");
    let (end_year, end_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(end_year, end_month, 1).unwrap();
    let last_day = first_next.pred_opt().unwrap();
    (start, last_day.format("%Y-%m-%d").to_string())
}

/// `YYYYMM` de `date_start` au mois courant inclus (port de `_list_target_months`).
pub(super) fn list_target_months(date_start_iso: &str, today: NaiveDate) -> Vec<String> {
    let start =
        NaiveDate::parse_from_str(date_start_iso, "%Y-%m-%d").expect("date_start ISO YYYY-MM-DD");
    let (mut y, mut m) = (start.year(), start.month() as i32);
    let (ty, tm) = (today.year(), today.month() as i32);
    let mut months = Vec::new();
    while (y, m) <= (ty, tm) {
        months.push(format!("{y:04}{m:02}"));
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    months
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_range_inclusive() {
        let today = NaiveDate::from_ymd_opt(2021, 3, 1).unwrap();
        assert_eq!(
            month_range("2021-01", today),
            vec!["202101", "202102", "202103"]
        );
    }

    #[test]
    fn month_bounds_handles_december_and_leap() {
        assert_eq!(
            month_bounds("202612"),
            ("2026-12-01".to_string(), "2026-12-31".to_string())
        );
        // Février bissextile.
        assert_eq!(
            month_bounds("202402"),
            ("2024-02-01".to_string(), "2024-02-29".to_string())
        );
        assert_eq!(
            month_bounds("202502"),
            ("2025-02-01".to_string(), "2025-02-28".to_string())
        );
    }

    #[test]
    fn list_target_months_from_start() {
        let today = NaiveDate::from_ymd_opt(2022, 7, 15).unwrap();
        assert_eq!(
            list_target_months("2022-05-28", today),
            vec!["202205", "202206", "202207"]
        );
    }
}
