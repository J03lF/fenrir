use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::errors::IdentityError;
use super::store::IdentityKeyMaterial;
use crate::utils::messages::security::identity as identity_messages;

pub(super) fn encode_jwt(
    key: &IdentityKeyMaterial,
    claims: &RawClaims,
) -> Result<String, IdentityError> {
    let header = RawHeader {
        alg: "EdDSA".to_string(),
        typ: "JWT".to_string(),
        kid: key.key_id.clone(),
    };
    let header_json = serde_json::to_vec(&header)?;
    let claims_json = serde_json::to_vec(claims)?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json);
    let claims_b64 = URL_SAFE_NO_PAD.encode(&claims_json);
    let signing_input = format!("{header_b64}.{claims_b64}");
    let keypair = key.keypair()?;
    let signature = keypair.sign(signing_input.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    Ok(format!("{signing_input}.{signature_b64}"))
}

pub(super) fn fingerprint_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

pub(super) fn parse_jwt(
    token: &str,
) -> Result<(ParsedHeader, ParsedClaims, Signature), IdentityError> {
    let mut parts = token.split('.');
    let header = parts.next().ok_or_else(|| {
        IdentityError::Unauthorized(identity_messages::invalid_token_missing_header().into())
    })?;
    let claims = parts.next().ok_or_else(|| {
        IdentityError::Unauthorized(identity_messages::invalid_token_missing_claims().into())
    })?;
    let signature = parts.next().ok_or_else(|| {
        IdentityError::Unauthorized(identity_messages::invalid_token_missing_signature().into())
    })?;
    if parts.next().is_some() {
        return Err(IdentityError::Unauthorized(
            identity_messages::invalid_token_too_many_segments().into(),
        ));
    }

    let header_bytes = URL_SAFE_NO_PAD.decode(header).map_err(|_| {
        IdentityError::Unauthorized(identity_messages::invalid_token_header_encoding().into())
    })?;
    let header_obj: Header = serde_json::from_slice(&header_bytes).map_err(|_| {
        IdentityError::Unauthorized(identity_messages::invalid_token_header_payload().into())
    })?;

    let claims_bytes = URL_SAFE_NO_PAD.decode(claims).map_err(|_| {
        IdentityError::Unauthorized(identity_messages::invalid_token_claims_encoding().into())
    })?;

    let signature_bytes = URL_SAFE_NO_PAD.decode(signature).map_err(|_| {
        IdentityError::Unauthorized(identity_messages::invalid_signature_encoding().into())
    })?;
    let signature = Signature::from_bytes(&signature_bytes).map_err(|_| {
        IdentityError::Unauthorized(identity_messages::invalid_signature_length().into())
    })?;

    Ok((
        ParsedHeader {
            encoded_header: header.to_string(),
            alg: header_obj.alg,
            kid: header_obj.kid,
        },
        ParsedClaims {
            encoded_claims: claims.to_string(),
            decoded: claims_bytes,
        },
        signature,
    ))
}

#[derive(Serialize, Deserialize)]
pub(super) struct RawHeader {
    pub(super) alg: String,
    pub(super) typ: String,
    pub(super) kid: String,
}

#[derive(Deserialize)]
pub(super) struct Header {
    pub(super) alg: String,
    #[allow(dead_code)]
    pub(super) typ: String,
    pub(super) kid: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct RawClaims {
    pub(super) iss: String,
    pub(super) aud: String,
    pub(super) sub: String,
    pub(super) env: String,
    pub(super) inst: String,
    pub(super) role: String,
    pub(super) ver: String,
    pub(super) iat: i64,
    pub(super) exp: i64,
    pub(super) jti: String,
}

#[derive(Deserialize)]
pub(super) struct Claims {
    pub(super) iss: String,
    pub(super) aud: String,
    pub(super) sub: String,
    pub(super) env: String,
    pub(super) inst: String,
    pub(super) role: String,
    pub(super) ver: String,
    pub(super) iat: i64,
    pub(super) exp: i64,
    pub(super) jti: String,
}

pub(super) struct ParsedHeader {
    pub(super) encoded_header: String,
    pub(super) alg: String,
    pub(super) kid: String,
}

pub(super) struct ParsedClaims {
    pub(super) encoded_claims: String,
    pub(super) decoded: Vec<u8>,
}
