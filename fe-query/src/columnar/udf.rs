use std::sync::Arc;

use datafusion::arrow::array::{Array, BooleanArray, Float64Array};
use datafusion::arrow::datatypes::DataType;
use datafusion::logical_expr::{ColumnarValue, Volatility};
use datafusion::prelude::SessionContext;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Haversine distance in meters between two EPSG:4326 (lat/lon) points.
pub fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (lat1, lon1, lat2, lon2) = (
        lat1.to_radians(),
        lon1.to_radians(),
        lat2.to_radians(),
        lon2.to_radians(),
    );
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

fn st_distance_impl(
    args: &[ColumnarValue],
) -> Result<ColumnarValue, datafusion::error::DataFusionError> {
    if args.len() != 4 {
        return Err(datafusion::error::DataFusionError::Plan(
            "st_distance requires 4 arguments: x1, y1, x2, y2".into(),
        ));
    }

    let arrays: Vec<_> = args
        .iter()
        .map(|a| match a {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(s) => s.to_array(),
        })
        .collect::<Result<_, _>>()?;

    let x1 = arrays[0]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_distance: x1 must be Float64".into())
        })?;
    let y1 = arrays[1]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_distance: y1 must be Float64".into())
        })?;
    let x2 = arrays[2]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_distance: x2 must be Float64".into())
        })?;
    let y2 = arrays[3]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_distance: y2 must be Float64".into())
        })?;

    let len = x1.len();
    let mut results = Vec::with_capacity(len);
    for i in 0..len {
        if x1.is_null(i) || y1.is_null(i) || x2.is_null(i) || y2.is_null(i) {
            results.push(f64::NAN);
        } else {
            results.push(haversine(
                x1.value(i),
                y1.value(i),
                x2.value(i),
                y2.value(i),
            ));
        }
    }

    Ok(ColumnarValue::Array(Arc::new(Float64Array::from(results))))
}

fn st_dwithin_impl(
    args: &[ColumnarValue],
) -> Result<ColumnarValue, datafusion::error::DataFusionError> {
    if args.len() != 5 {
        return Err(datafusion::error::DataFusionError::Plan(
            "st_dwithin requires 5 arguments: x1, y1, x2, y2, distance_m".into(),
        ));
    }

    let arrays: Vec<_> = args
        .iter()
        .map(|a| match a {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(s) => s.to_array(),
        })
        .collect::<Result<_, _>>()?;

    let x1 = arrays[0]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_dwithin: x1 must be Float64".into())
        })?;
    let y1 = arrays[1]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_dwithin: y1 must be Float64".into())
        })?;
    let x2 = arrays[2]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_dwithin: x2 must be Float64".into())
        })?;
    let y2 = arrays[3]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan("st_dwithin: y2 must be Float64".into())
        })?;
    let dist = arrays[4]
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| {
            datafusion::error::DataFusionError::Plan(
                "st_dwithin: distance_m must be Float64".into(),
            )
        })?;

    let len = x1.len();
    let mut results = Vec::with_capacity(len);
    for i in 0..len {
        if x1.is_null(i) || y1.is_null(i) || x2.is_null(i) || y2.is_null(i) || dist.is_null(i) {
            results.push(false);
        } else {
            let d = haversine(x1.value(i), y1.value(i), x2.value(i), y2.value(i));
            results.push(d <= dist.value(i));
        }
    }

    Ok(ColumnarValue::Array(Arc::new(BooleanArray::from(results))))
}

/// Register spatial UDFs with a DataFusion `SessionContext`.
pub fn register_spatial_udfs(ctx: &SessionContext) {
    let st_distance = datafusion::logical_expr::ScalarUDF::new_from_impl(StDistanceUdf);
    ctx.register_udf(st_distance);

    let st_dwithin = datafusion::logical_expr::ScalarUDF::new_from_impl(StDwithinUdf);
    ctx.register_udf(st_dwithin);
}

#[derive(Debug)]
struct StDistanceUdf;

impl datafusion::logical_expr::ScalarUDFImpl for StDistanceUdf {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "st_distance"
    }

    fn signature(&self) -> &datafusion::logical_expr::Signature {
        static SIG: std::sync::OnceLock<datafusion::logical_expr::Signature> =
            std::sync::OnceLock::new();
        SIG.get_or_init(|| {
            datafusion::logical_expr::Signature::exact(
                vec![
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                ],
                Volatility::Immutable,
            )
        })
    }

    fn return_type(
        &self,
        _arg_types: &[DataType],
    ) -> Result<DataType, datafusion::error::DataFusionError> {
        Ok(DataType::Float64)
    }

    fn invoke_batch(
        &self,
        args: &[ColumnarValue],
        _number_rows: usize,
    ) -> Result<ColumnarValue, datafusion::error::DataFusionError> {
        st_distance_impl(args)
    }
}

#[derive(Debug)]
struct StDwithinUdf;

impl datafusion::logical_expr::ScalarUDFImpl for StDwithinUdf {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "st_dwithin"
    }

    fn signature(&self) -> &datafusion::logical_expr::Signature {
        static SIG: std::sync::OnceLock<datafusion::logical_expr::Signature> =
            std::sync::OnceLock::new();
        SIG.get_or_init(|| {
            datafusion::logical_expr::Signature::exact(
                vec![
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                ],
                Volatility::Immutable,
            )
        })
    }

    fn return_type(
        &self,
        _arg_types: &[DataType],
    ) -> Result<DataType, datafusion::error::DataFusionError> {
        Ok(DataType::Boolean)
    }

    fn invoke_batch(
        &self,
        args: &[ColumnarValue],
        _number_rows: usize,
    ) -> Result<ColumnarValue, datafusion::error::DataFusionError> {
        st_dwithin_impl(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_zero_distance() {
        let d = haversine(51.5074, -0.1278, 51.5074, -0.1278);
        assert!(d.abs() < 1e-6);
    }

    #[test]
    fn haversine_london_to_paris() {
        // London (51.5074, -0.1278) to Paris (48.8566, 2.3522) ~ 343 km
        let d = haversine(51.5074, -0.1278, 48.8566, 2.3522);
        assert!((d - 343_000.0).abs() < 5_000.0, "expected ~343km, got {d}");
    }

    #[test]
    fn st_distance_impl_basic() {
        let args = vec![
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![51.5074]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![-0.1278]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![48.8566]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![2.3522]))),
        ];
        let result = st_distance_impl(&args).unwrap();
        match result {
            ColumnarValue::Array(arr) => {
                let f = arr.as_any().downcast_ref::<Float64Array>().unwrap();
                assert!((f.value(0) - 343_000.0).abs() < 5_000.0);
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn st_dwithin_impl_basic() {
        let args = vec![
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.001]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![200.0]))),
        ];
        let result = st_dwithin_impl(&args).unwrap();
        match result {
            ColumnarValue::Array(arr) => {
                let b = arr.as_any().downcast_ref::<BooleanArray>().unwrap();
                assert!(b.value(0));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn st_dwithin_beyond_range() {
        let args = vec![
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![0.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![1.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![1.0]))),
            ColumnarValue::Array(Arc::new(Float64Array::from(vec![100.0]))),
        ];
        let result = st_dwithin_impl(&args).unwrap();
        match result {
            ColumnarValue::Array(arr) => {
                let b = arr.as_any().downcast_ref::<BooleanArray>().unwrap();
                assert!(!b.value(0));
            }
            _ => panic!("expected array"),
        }
    }
}
