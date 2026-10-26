//! The diagnostics of this crate's errors: table metadata and manifests
//! with the codes of the store category, a read that cannot be planned
//! with those of schema.

use utilake_diag::{Category, Diagnose, Diagnostic};

use crate::table::avro::AvroError;
use crate::{PlanError, PublishError};

/// A read that cannot be planned across the table's schemas.
impl Diagnose for PlanError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::UnknownSchema(_) => {
                Diagnostic::error("schema.unknown_schema_id", Category::SCHEMA, msg)
            }
            Self::NotAPromotion { name, from, to, .. } => {
                Diagnostic::error("schema.not_a_promotion", Category::SCHEMA, msg)
                    .with_column(name.clone())
                    .expected_observed(format!("a type {from} promotes to"), to.to_string())
            }
        }
    }
}

/// Table metadata that cannot be written or read back.
impl Diagnose for PublishError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("store.iceberg_metadata", Category::STORE, self.to_string())
    }
}

/// A manifest or manifest list that cannot be written or read.
impl Diagnose for AvroError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("store.iceberg_manifest", Category::STORE, self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IcebergType;

    #[test]
    fn every_error_has_its_code() {
        assert_eq!(
            PlanError::UnknownSchema(3).diagnostic().code,
            "schema.unknown_schema_id"
        );
        let d = PlanError::NotAPromotion {
            name: "C".into(),
            schema: 1,
            from: IcebergType::Double,
            to: IcebergType::Long,
        }
        .diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.column.as_deref(), d.observed.as_deref()),
            ("schema.not_a_promotion", Some("C"), Some("long"))
        );
        assert_eq!(
            PublishError::UnknownSchema(2).diagnostic().code,
            "store.iceberg_metadata"
        );
        assert_eq!(
            AvroError::Truncated.diagnostic().code,
            "store.iceberg_manifest"
        );
    }
}
