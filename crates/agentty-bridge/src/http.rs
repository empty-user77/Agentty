//! Shared HTTP client setup.
//!
//! Certificates are checked against the bundled Mozilla roots first, exactly as before. Only when
//! that check fails is the operating system asked (Security.framework on macOS, as Safari does),
//! so networks that inspect HTTPS with their own root certificate — company VPNs and proxies
//! whose root is trusted in the keychain or installed by an MDM profile — work too, while every
//! other connection is verified the same way it always was.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use std::sync::{Arc, OnceLock};

/// An agent builder with Agentty's TLS configuration.
pub fn agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().tls_config(tls_config())
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let verifier = BundledThenSystem::new(provider.clone());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
                .expect("TLS 1.2 and 1.3 are supported by ring")
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Bundled roots first; the OS verifier only for certificates the bundled roots reject.
#[derive(Debug)]
struct BundledThenSystem {
    bundled: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
    system: OnceLock<Option<rustls_platform_verifier::Verifier>>,
}

impl BundledThenSystem {
    fn new(provider: Arc<CryptoProvider>) -> Self {
        let roots = Arc::new(RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() });
        let bundled = WebPkiServerVerifier::builder_with_provider(roots, provider.clone()).build().expect("the bundled roots are valid");
        Self { bundled, provider, system: OnceLock::new() }
    }

    fn system(&self) -> Option<&rustls_platform_verifier::Verifier> {
        self.system.get_or_init(|| rustls_platform_verifier::Verifier::new(self.provider.clone()).ok()).as_ref()
    }
}

impl ServerCertVerifier for BundledThenSystem {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let bundled = self.bundled.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now);
        let Err(err) = bundled else { return bundled };
        match self.system() {
            // The original error is kept when the OS rejects the certificate too.
            Some(system) => system.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now).map_err(|_| err),
            None => Err(err),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.bundled.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.bundled.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.bundled.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_verifier_is_available() {
        let verifier = BundledThenSystem::new(Arc::new(rustls::crypto::ring::default_provider()));
        assert!(verifier.system().is_some());
    }

    #[test]
    #[ignore = "needs network"]
    fn verifies_certificates_over_https() {
        let agent = agent_builder().build();
        let status = |url: &str| agent.get(url).set("User-Agent", "Agentty-test").call().map(|r| r.status()).ok();
        assert_eq!(status("https://api.github.com/zen"), Some(200));
        for bad in [
            "https://self-signed.badssl.com/",
            "https://untrusted-root.badssl.com/",
            "https://expired.badssl.com/",
            "https://wrong.host.badssl.com/",
        ] {
            assert_eq!(status(bad), None, "{bad} was accepted");
        }
    }
}
