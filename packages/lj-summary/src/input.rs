//! The decision's title and body as the model reads them.

fn jurisdiction_label(jurisdiction_type: &str) -> Option<&'static str> {
    Some(match jurisdiction_type {
        "TA" => "Tribunal administratif",
        "CAA" => "Cour administrative d'appel",
        "CE" => "Conseil d'État",
        "CC" => "Cour de cassation",
        "CA" => "Cour d'appel",
        "TJ" => "Tribunal judiciaire",
        "TCOM" => "Tribunal de commerce",
        _ => return None,
    })
}

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

fn format_fr_date(date_lecture: &str) -> String {
    let parts: Vec<&str> = date_lecture.split('-').collect();
    if parts.len() != 3 {
        return date_lecture.to_string();
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        parts[0].parse::<i32>(),
        parts[1].parse::<usize>(),
        parts[2].parse::<i32>(),
    ) else {
        return date_lecture.to_string();
    };
    match FR_MONTHS.get(month.wrapping_sub(1)) {
        Some(m) => format!("{day} {m} {year}"),
        None => date_lecture.to_string(),
    }
}

fn decision_jurisdiction(jurisdiction_type: &str, jurisdiction_name: Option<&str>) -> String {
    if let Some(name) = jurisdiction_name {
        let cleaned = name.trim().replace(" ,", ",");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    jurisdiction_label(jurisdiction_type)
        .unwrap_or(jurisdiction_type)
        .to_string()
}

pub fn decision_title(
    jurisdiction_type: &str,
    jurisdiction_name: Option<&str>,
    date_lecture: Option<&str>,
    docket_numbers: Option<&[String]>,
) -> String {
    let mut parts = vec![decision_jurisdiction(jurisdiction_type, jurisdiction_name)];
    if let Some(date) = date_lecture {
        parts.push(format_fr_date(date));
    }
    if let Some(dockets) = docket_numbers {
        if let Some(first) = dockets.first() {
            parts.push(first.clone());
        }
    }
    parts.join(", ")
}

pub fn body_text_from_full_text(full_text: &str) -> Option<String> {
    let paragraphs: Vec<&str> = full_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    Some(paragraphs.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_date() {
        assert_eq!(format_fr_date("2026-05-29"), "29 mai 2026");
        assert_eq!(format_fr_date("2026-01-01"), "1 janvier 2026");
        assert_eq!(format_fr_date("2026-12-31"), "31 décembre 2026");
        assert_eq!(format_fr_date("pas-une-date-x"), "pas-une-date-x");
        assert_eq!(format_fr_date("2026/05/29"), "2026/05/29");
    }

    #[test]
    fn title_full() {
        let t = decision_title(
            "CC",
            Some("Cour de cassation"),
            Some("2026-05-29"),
            Some(&["24-17.384".to_string()]),
        );
        assert_eq!(t, "Cour de cassation, 29 mai 2026, 24-17.384");
    }

    #[test]
    fn title_falls_back_to_label() {
        let t = decision_title("TA", None, Some("2026-05-29"), None);
        assert_eq!(t, "Tribunal administratif, 29 mai 2026");
    }

    #[test]
    fn jurisdiction_sanitised() {
        assert_eq!(
            decision_jurisdiction("TA", Some("  Tribunal administratif de Paris ,  ")),
            "Tribunal administratif de Paris,"
        );
        assert_eq!(decision_jurisdiction("CE", Some("   ")), "Conseil d'État");
    }
}
