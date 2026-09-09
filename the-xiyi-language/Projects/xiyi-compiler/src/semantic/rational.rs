// src/semantic/rational.rs
use super::check_program::TypeChecker;

impl TypeChecker {
    pub fn parse_rational(s: &str) -> Option<(i128, u128)> {
        let s = s.trim();
        if s.contains('/') {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() == 2 {
                let num = parts[0].trim().parse::<i128>().ok()?;
                let den = parts[1].trim().parse::<u128>().ok()?;
                if den == 0 { return None; }
                return Some((num, den));
            }
            None
        } else if let Ok(num) = s.parse::<i128>() {
            Some((num, 1))
        } else if let Some((int_part, frac_part)) = s.split_once('.') {
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let int_val = if int_part.is_empty() || int_part == "-" {
                0
            } else {
                int_part.parse::<i128>().ok()?
            };
            let frac_str = frac_part.trim_end_matches('0');
            let den = 10u128.pow(frac_str.len() as u32);
            let frac_val = if frac_str.is_empty() { 0 } else { frac_str.parse::<i128>().ok()? };
            let num = int_val * den as i128 + sign * frac_val;
            Some((num, den))
        } else {
            None
        }
    }

    pub fn rational_le(a: &str, b: &str) -> bool {
        let (na, da) = Self::parse_rational(a).unwrap_or((0, 1));
        let (nb, db) = Self::parse_rational(b).unwrap_or((0, 1));
        na * db as i128 <= nb * da as i128
    }

    pub fn rational_min(a: &str, b: &str) -> String {
        if Self::rational_le(a, b) { a.to_string() } else { b.to_string() }
    }

    pub fn rational_eq(a: &str, b: &str) -> bool {
        let (na, da) = Self::parse_rational(a).unwrap_or((0, 1));
        let (nb, db) = Self::parse_rational(b).unwrap_or((0, 1));
        na * db as i128 == nb * da as i128
    }
}
