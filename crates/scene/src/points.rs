//! Serde forms for points. Everything geometric (bounds, hit testing, outlines, snapping) is the `geom` crate's job.

/// Serde for one [`Point`]: written as `[x, y]`, read from `[x, y]` or `{"x": x, "y": y}`.
pub mod pt {
    use kurbo::Point;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Pair([f64; 2]),
        Obj { x: f64, y: f64 },
    }

    /// Serialises as `[x, y]`.
    pub fn serialize<S: Serializer>(p: &Point, s: S) -> Result<S::Ok, S::Error> {
        [p.x, p.y].serialize(s)
    }

    /// Deserialises `[x, y]` or `{x, y}`.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Point, D::Error> {
        Ok(match Repr::deserialize(d)? {
            Repr::Pair([x, y]) | Repr::Obj { x, y } => Point::new(x, y),
        })
    }
}

/// Serde for `Vec<Point>`, same forms as [`pt`].
pub mod pts {
    use kurbo::Point;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialises as `[[x, y], ...]`.
    pub fn serialize<S: Serializer>(v: &[Point], s: S) -> Result<S::Ok, S::Error> {
        let pairs: Vec<[f64; 2]> = v.iter().map(|p| [p.x, p.y]).collect();
        pairs.serialize(s)
    }

    #[derive(Deserialize)]
    struct One(#[serde(with = "super::pt")] Point);

    /// Deserialises a list of `[x, y]` or `{x, y}`.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
        Ok(Vec::<One>::deserialize(d)?.into_iter().map(|p| p.0).collect())
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Point;

    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct W {
        #[serde(with = "pt")]
        p: Point,
        #[serde(with = "pts")]
        v: Vec<Point>,
    }

    #[test]
    fn point_forms() {
        let w: W = serde_json::from_str(r#"{"p":[1,2],"v":[[3,4],{"x":5,"y":6}]}"#).unwrap();
        assert_eq!(w.p, Point::new(1.0, 2.0));
        assert_eq!(w.v, vec![Point::new(3.0, 4.0), Point::new(5.0, 6.0)]);
        assert_eq!(serde_json::to_string(&w).unwrap(), r#"{"p":[1.0,2.0],"v":[[3.0,4.0],[5.0,6.0]]}"#);
        assert!(serde_json::from_str::<W>(r#"{"p":[1],"v":[]}"#).is_err());
        assert!(serde_json::from_str::<W>(r#"{"p":[1,2,3],"v":[]}"#).is_err());
        assert!(serde_json::from_str::<W>(r#"{"p":"a","v":[]}"#).is_err());
    }
}
