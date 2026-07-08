//! Logique pure du scroll-spy du sommaire décision. Port de
//! `apps/web/src/lib/decision-toc-spy.ts`. Testable hors DOM.

/// `scroll-margin-top` des ancres de section (= `scroll-mt-20`, 80px). Ligne
/// d'atterrissage après clic / jump `#hash`.
pub const ANCHOR_SCROLL_MARGIN_PX: f64 = 80.0;

/// Biais d'activation : la ligne de lecture du spy est posée quelques px sous la
/// ligne d'atterrissage.
const ACTIVATION_BIAS_PX: f64 = 12.0;

/// Décalage entre le haut du viewport et la ligne de lecture (atterrissage +
/// biais).
pub const SCROLL_SPY_OFFSET_PX: f64 = ANCHOR_SCROLL_MARGIN_PX + ACTIVATION_BIAS_PX;

/// Métrique d'une section : ancre absolue + centre de l'item dans la liste.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyMetric {
    pub id: String,
    pub anchor_top: f64,
    pub center: f64,
}

/// Résultat : section active + position de la barre de progression.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyResult {
    pub id: String,
    pub progress: f64,
}

/// Section active + position de la barre pour une ligne de marqueur donnée.
/// Port de `resolveScrollSpy`. `metrics` triés par `anchor_top`.
pub fn resolve_scroll_spy(
    metrics: &[SpyMetric],
    marker_y: f64,
    at_bottom: bool,
) -> Option<SpyResult> {
    if at_bottom {
        return metrics.last().map(|last| SpyResult {
            id: last.id.clone(),
            progress: last.center,
        });
    }

    let mut current: Option<&SpyMetric> = None;
    let mut next: Option<&SpyMetric> = None;
    for (index, metric) in metrics.iter().enumerate() {
        if marker_y >= metric.anchor_top || current.is_none() {
            current = Some(metric);
            next = metrics.get(index + 1);
        } else {
            break;
        }
    }

    let current = current?;
    let anchor_next = next.unwrap_or(current);
    let range = anchor_next.anchor_top - current.anchor_top;
    let blend = if range > 0.0 {
        ((marker_y - current.anchor_top) / range).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let progress = current.center + (anchor_next.center - current.center) * blend;
    Some(SpyResult {
        id: current.id.clone(),
        progress,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(id: &str, anchor_top: f64, center: f64) -> SpyMetric {
        SpyMetric {
            id: id.to_string(),
            anchor_top,
            center,
        }
    }

    #[test]
    fn at_bottom_forces_last() {
        let m = vec![metric("a", 0.0, 10.0), metric("b", 100.0, 30.0)];
        let res = resolve_scroll_spy(&m, 5.0, true).unwrap();
        assert_eq!(res.id, "b");
        assert_eq!(res.progress, 30.0);
    }

    #[test]
    fn picks_last_anchor_above_marker_and_interpolates() {
        let m = vec![
            metric("a", 0.0, 10.0),
            metric("b", 100.0, 30.0),
            metric("c", 300.0, 50.0),
        ];
        // marker between a and b → active a, blend 0.5.
        let res = resolve_scroll_spy(&m, 50.0, false).unwrap();
        assert_eq!(res.id, "a");
        assert!((res.progress - 20.0).abs() < 1e-9);
    }

    #[test]
    fn empty_metrics_none() {
        assert!(resolve_scroll_spy(&[], 0.0, false).is_none());
    }
}
