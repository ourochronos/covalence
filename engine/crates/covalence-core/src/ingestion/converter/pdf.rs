//! PDF converter via an external sidecar.

use crate::error::{Error, Result};
use crate::ingestion::converter::SourceConverter;

/// Converter for PDF content via an external sidecar.
///
/// Calls a configurable HTTP endpoint (e.g., pymupdf4llm sidecar)
/// to extract Markdown from PDF files. The sidecar must accept
/// `POST /convert-pdf` with raw PDF bytes in the body and return
/// `{"markdown": "..."}`.
pub struct PdfConverter {
    /// Base URL of the PDF conversion sidecar.
    base_url: String,
    /// HTTP client with generous timeout for large PDFs.
    client: reqwest::Client,
}

impl PdfConverter {
    /// Create a new PDF converter pointing at the given sidecar URL.
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self { base_url, client }
    }

    /// Validate connectivity with the PDF sidecar.
    ///
    /// Calls the health endpoint to verify the sidecar is reachable
    /// and ready. Returns an error with a clear message if not.
    pub async fn validate(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                Error::Ingestion(format!(
                    "PDF sidecar validation failed ({}): {e}",
                    self.base_url
                ))
            })?;

        if !resp.status().is_success() {
            return Err(Error::Ingestion(format!(
                "PDF sidecar health check returned {}",
                resp.status()
            )));
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl SourceConverter for PdfConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        let url = format!("{}/convert-pdf", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/pdf")
            .body(content.to_vec())
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("PDF sidecar request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "PDF sidecar returned {status}: {body_text}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse PDF response: {e}")))?;

        json["markdown"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::Ingestion("PDF sidecar response missing 'markdown' field".to_string())
            })
    }

    fn supported_types(&self) -> &[&str] {
        &["application/pdf"]
    }
}
