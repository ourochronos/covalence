//! Subjective Logic opinion tuples.
//!
//! An opinion `w = (b, d, u, a)` represents a belief state where:
//! - `b` = belief (positive evidence)
//! - `d` = disbelief (negative evidence)
//! - `u` = uncertainty (ignorance)
//! - `a` = base rate (prior probability absent evidence)
//! - Constraint: `b + d + u = 1`

use serde::{Deserialize, Serialize};

/// Tolerance for floating-point comparisons.
const EPSILON: f64 = 1e-6;

/// A Subjective Logic opinion tuple.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Opinion {
    /// Degree of positive evidence.
    pub belief: f64,

    /// Degree of negative evidence.
    pub disbelief: f64,

    /// Degree of ignorance.
    pub uncertainty: f64,

    /// Prior probability absent evidence.
    pub base_rate: f64,
}

impl Opinion {
    /// Create a new opinion. Returns None if `b + d + u != 1` (within tolerance).
    pub fn new(belief: f64, disbelief: f64, uncertainty: f64, base_rate: f64) -> Option<Self> {
        let sum = belief + disbelief + uncertainty;
        if (sum - 1.0).abs() > EPSILON {
            return None;
        }
        Some(Self {
            belief,
            disbelief,
            uncertainty,
            base_rate,
        })
    }

    /// Create a vacuous opinion (complete ignorance).
    pub fn vacuous(base_rate: f64) -> Self {
        Self {
            belief: 0.0,
            disbelief: 0.0,
            uncertainty: 1.0,
            base_rate,
        }
    }

    /// Create a dogmatic opinion (complete certainty).
    pub fn certain(belief: f64) -> Self {
        Self {
            belief,
            disbelief: 1.0 - belief,
            uncertainty: 0.0,
            base_rate: 0.5,
        }
    }

    /// Projected probability: `b + a * u`.
    ///
    /// Provides backward compatibility with systems expecting a single float.
    pub fn projected_probability(&self) -> f64 {
        self.belief + self.base_rate * self.uncertainty
    }

    /// Cumulative fusion operator (`self + other`).
    ///
    /// Combines two independent opinions about the same proposition.
    /// When both opinions have zero uncertainty, falls back to averaging.
    /// Preserves commutativity and associativity.
    pub fn cumulative_fuse(&self, other: &Opinion) -> Opinion {
        let u_a = self.uncertainty;
        let u_b = other.uncertainty;

        // Both dogmatic: average the beliefs.
        if u_a < EPSILON && u_b < EPSILON {
            return self.average_fuse(other);
        }

        let denom = u_a + u_b - u_a * u_b;

        let b = (self.belief * u_b + other.belief * u_a) / denom;
        let d = (self.disbelief * u_b + other.disbelief * u_a) / denom;
        let u = (u_a * u_b) / denom;

        // Fused base rate: weighted average by the other's uncertainty.
        let a = if (u_a + u_b) < EPSILON {
            (self.base_rate + other.base_rate) / 2.0
        } else {
            (self.base_rate * u_b + other.base_rate * u_a) / (u_a + u_b)
        };

        Opinion {
            belief: b,
            disbelief: d,
            uncertainty: u,
            base_rate: a,
        }
    }

    /// Averaging fusion operator.
    ///
    /// Simple average of two opinions, useful when independence cannot be
    /// assumed or as a fallback for two dogmatic opinions.
    pub fn average_fuse(&self, other: &Opinion) -> Opinion {
        let u_a = self.uncertainty;
        let u_b = other.uncertainty;

        // Both dogmatic: simple average.
        if u_a < EPSILON && u_b < EPSILON {
            return Opinion {
                belief: (self.belief + other.belief) / 2.0,
                disbelief: (self.disbelief + other.disbelief) / 2.0,
                uncertainty: 0.0,
                base_rate: (self.base_rate + other.base_rate) / 2.0,
            };
        }

        let denom = u_a + u_b;

        let b = (self.belief * u_b + other.belief * u_a) / denom;
        let d = (self.disbelief * u_b + other.disbelief * u_a) / denom;
        let u = 2.0 * u_a * u_b / denom;

        let a = (self.base_rate + other.base_rate) / 2.0;

        Opinion {
            belief: b,
            disbelief: d,
            uncertainty: u,
            base_rate: a,
        }
    }

    /// Discount this opinion by a trust factor.
    ///
    /// The trust factor `t` (in `[0, 1]`) scales belief and disbelief,
    /// transferring the remainder to uncertainty. A fully trusted source
    /// (`t = 1.0`) returns the opinion unchanged; a fully distrusted source
    /// (`t = 0.0`) returns a vacuous opinion.
    pub fn discount(&self, trust: f64) -> Opinion {
        let t = trust.clamp(0.0, 1.0);
        Opinion {
            belief: t * self.belief,
            disbelief: t * self.disbelief,
            uncertainty: 1.0 - t * (self.belief + self.disbelief),
            base_rate: self.base_rate,
        }
    }

    /// Deduction operator: derive a conditional opinion.
    ///
    /// Given `self` as the opinion on proposition X and `if_true` / `if_false`
    /// as the conditional opinions on Y given X is true/false respectively,
    /// compute the derived opinion on Y.
    pub fn deduce(&self, if_true: &Opinion, if_false: &Opinion) -> Opinion {
        let p_x = self.projected_probability();

        let b_y = p_x * if_true.belief + (1.0 - p_x) * if_false.belief;
        let d_y = p_x * if_true.disbelief + (1.0 - p_x) * if_false.disbelief;
        let u_y = p_x * if_true.uncertainty + (1.0 - p_x) * if_false.uncertainty;
        let a_y = p_x * if_true.base_rate + (1.0 - p_x) * if_false.base_rate;

        // Renormalize to ensure b + d + u = 1.
        let sum = b_y + d_y + u_y;
        if sum < EPSILON {
            return Opinion::vacuous(a_y);
        }

        Opinion {
            belief: b_y / sum,
            disbelief: d_y / sum,
            uncertainty: u_y / sum,
            base_rate: a_y,
        }
    }

    /// Serialize to a JSON value for JSONB storage.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "belief": self.belief,
            "disbelief": self.disbelief,
            "uncertainty": self.uncertainty,
            "base_rate": self.base_rate,
            "projected_probability": self.projected_probability()
        })
    }

    /// Deserialize from a JSON value (as stored in JSONB).
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let belief = value.get("belief")?.as_f64()?;
        let disbelief = value.get("disbelief")?.as_f64()?;
        let uncertainty = value.get("uncertainty")?.as_f64()?;
        let base_rate = value.get("base_rate")?.as_f64()?;
        Self::new(belief, disbelief, uncertainty, base_rate)
    }
}

impl Default for Opinion {
    fn default() -> Self {
        Self::vacuous(0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projected_probability() {
        let o = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let pp = o.projected_probability();
        assert!((pp - 0.8).abs() < EPSILON);
    }

    #[test]
    fn test_vacuous_projected() {
        let o = Opinion::vacuous(0.5);
        let pp = o.projected_probability();
        assert!((pp - 0.5).abs() < EPSILON);
    }

    #[test]
    fn test_cumulative_fusion_reduces_uncertainty() {
        let a = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        let b = Opinion::new(0.6, 0.1, 0.3, 0.5).unwrap();
        let fused = a.cumulative_fuse(&b);
        assert!(fused.uncertainty < a.uncertainty);
        assert!(fused.uncertainty < b.uncertainty);
        assert!((fused.belief + fused.disbelief + fused.uncertainty - 1.0).abs() < EPSILON);
    }

    #[test]
    fn test_cumulative_fusion_commutative() {
        let a = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        let b = Opinion::new(0.3, 0.2, 0.5, 0.5).unwrap();
        let ab = a.cumulative_fuse(&b);
        let ba = b.cumulative_fuse(&a);
        assert!((ab.belief - ba.belief).abs() < EPSILON);
        assert!((ab.disbelief - ba.disbelief).abs() < EPSILON);
        assert!((ab.uncertainty - ba.uncertainty).abs() < EPSILON);
    }

    #[test]
    fn test_discount_full_trust() {
        let o = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let d = o.discount(1.0);
        assert!((d.belief - o.belief).abs() < EPSILON);
        assert!((d.disbelief - o.disbelief).abs() < EPSILON);
        assert!((d.uncertainty - o.uncertainty).abs() < EPSILON);
    }

    #[test]
    fn test_discount_zero_trust() {
        let o = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let d = o.discount(0.0);
        assert!(d.belief.abs() < EPSILON);
        assert!(d.disbelief.abs() < EPSILON);
        assert!((d.uncertainty - 1.0).abs() < EPSILON);
    }

    #[test]
    fn test_json_roundtrip() {
        let o = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let json = o.to_json();
        let restored = Opinion::from_json(&json).unwrap();
        assert!((o.belief - restored.belief).abs() < EPSILON);
        assert!((o.disbelief - restored.disbelief).abs() < EPSILON);
        assert!((o.uncertainty - restored.uncertainty).abs() < EPSILON);
        assert!((o.base_rate - restored.base_rate).abs() < EPSILON);
    }

    #[test]
    fn test_invalid_opinion() {
        assert!(Opinion::new(0.5, 0.5, 0.5, 0.5).is_none());
    }

    #[test]
    fn test_dogmatic_fusion_falls_back_to_average() {
        let a = Opinion::certain(0.8);
        let b = Opinion::certain(0.6);
        let fused = a.cumulative_fuse(&b);
        assert!((fused.belief - 0.7).abs() < EPSILON);
        assert!(fused.uncertainty.abs() < EPSILON);
    }
}
