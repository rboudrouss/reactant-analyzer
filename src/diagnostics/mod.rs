use crate::events::SourceLocation;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelatedLoc {
    pub message: String,
    pub loc: SerializedLoc,
}

#[derive(Debug, Clone, Serialize)]
pub struct SerializedLoc {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl From<&SourceLocation> for SerializedLoc {
    fn from(l: &SourceLocation) -> Self {
        SerializedLoc {
            file: l.file.clone(),
            line: l.line,
            column: l.column,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub rule_id: &'static str,
    pub severity: Severity,
    pub message: String,
    pub component_name: String,
    pub loc: SerializedLoc,
    pub related: Vec<RelatedLoc>,
}

impl Warning {
    pub fn new(
        rule_id: &'static str,
        severity: Severity,
        message: String,
        component_name: String,
        loc: &SourceLocation,
    ) -> Self {
        Warning {
            rule_id,
            severity,
            message,
            component_name,
            loc: loc.into(),
            related: vec![],
        }
    }

    pub fn with_related(mut self, message: String, loc: &SourceLocation) -> Self {
        self.related.push(RelatedLoc {
            message,
            loc: loc.into(),
        });
        self
    }
}
