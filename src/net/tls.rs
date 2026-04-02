pub use embedded_tls::NoVerify;

// We re-export specialized types if needed, but for now using the built-in generic NoVerify is sufficient.
// To implement a custom verifier, we'd need to find where CertificateRef and CertificateVerify are re-exported
// or use the 'webpki' feature which provides its own verifiers.
