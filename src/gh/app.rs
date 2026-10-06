//! Built-in GitHub App authentication.
//!
//! An installation token minted by `actions/create-github-app-token` dies
//! after one hour, and that is a ceiling on every wait gh-ship does. With the
//! App's credentials in the environment, gh-ship mints the token itself and
//! mints a fresh one shortly before the old one expires, so a long publish
//! build no longer races the clock.
//!
//! This is strictly opt-in: with neither `SHIP_APP_CLIENT_ID` nor
//! `SHIP_APP_PRIVATE_KEY` set, none of this runs and authentication stays
//! `gh`'s job.
//!
//! gh-ship still implements no HTTP client. The two calls it needs — find the
//! installation, then exchange a JWT for an installation token — go through
//! `gh api`, authenticated with the JWT instead of a token.

use std::fmt;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use serde::Deserialize;
use sha2::Sha256;

use super::cli::GhError;

/// How long GitHub keeps an installation token alive.
///
/// Fixed by GitHub, and assumed rather than read back from `expires_at`:
/// measuring from our own monotonic clock cannot be fooled by a runner whose
/// wall clock disagrees with GitHub's.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// How long before expiry a token is replaced.
///
/// Enough that a request started just before the cut-over still finishes on
/// a valid token.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The lifetime of the JWT used to ask for an installation token.
///
/// GitHub caps it at ten minutes; the token is only ever used for the two
/// calls made immediately, so there is no reason to approach the cap.
const JWT_LIFETIME: u64 = 9 * 60;

/// How far back the JWT's `iat` is set, to absorb clock drift.
const JWT_BACKDATE: u64 = 60;

/// The assumed lifetime of an installation token, overridable via
/// `SHIP_APP_TOKEN_TTL` (seconds).
///
/// A test knob: it lets the suite watch a token being replaced without
/// waiting an hour.
fn token_lifetime() -> Duration {
    super::env_duration("SHIP_APP_TOKEN_TTL").unwrap_or(TOKEN_LIFETIME)
}

/// Whether a token of the given age should be replaced.
///
/// The margin never exceeds a quarter of the lifetime, so a short lifetime
/// is still usable for most of its length.
pub(crate) fn refresh_due(age: Duration, lifetime: Duration) -> bool {
    let margin = std::cmp::min(REFRESH_MARGIN, lifetime / 4);
    age + margin >= lifetime
}

struct Minted {
    token: String,
    at: Instant,
}

/// GitHub App credentials plus the installation token minted from them.
pub struct AppAuth {
    client_id: Option<String>,
    private_key: Option<String>,
    installation_id: Option<String>,
    /// `OWNER/REPO`, to find the installation and scope the token.
    repo: Option<String>,
    cache: Mutex<Option<Minted>>,
}

// Hand-written so that neither the key nor a live token can reach a log.
impl fmt::Debug for AppAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppAuth")
            .field("client_id", &self.client_id)
            .field("installation_id", &self.installation_id)
            .field("repo", &self.repo)
            .finish_non_exhaustive()
    }
}

impl AppAuth {
    /// Read the App credentials from the environment.
    ///
    /// `None` when neither credential is set, which is the normal case. A
    /// half-configured pair is still `Some`: it fails when a token is first
    /// needed, with a message that names what is missing, rather than
    /// quietly falling back to whatever `gh` finds.
    pub fn from_env(repo: Option<&str>) -> Option<Self> {
        let client_id = env_nonempty("SHIP_APP_CLIENT_ID");
        let private_key = env_nonempty("SHIP_APP_PRIVATE_KEY");
        if client_id.is_none() && private_key.is_none() {
            return None;
        }

        let repo = repo
            .map(str::to_string)
            .or_else(|| env_nonempty("GITHUB_REPOSITORY"));

        Some(Self {
            client_id,
            private_key,
            installation_id: env_nonempty("SHIP_APP_INSTALLATION_ID"),
            repo,
            cache: Mutex::new(None),
        })
    }

    #[cfg(test)]
    fn new(
        client_id: Option<&str>,
        private_key: Option<&str>,
        installation_id: Option<&str>,
        repo: Option<&str>,
    ) -> Self {
        Self {
            client_id: client_id.map(str::to_string),
            private_key: private_key.map(str::to_string),
            installation_id: installation_id.map(str::to_string),
            repo: repo.map(str::to_string),
            cache: Mutex::new(None),
        }
    }

    /// A valid installation token, minting a new one when due.
    pub fn token(&self) -> Result<String, GhError> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(minted) = cache.as_ref()
            && !refresh_due(minted.at.elapsed(), token_lifetime())
        {
            return Ok(minted.token.clone());
        }

        let token = self.mint()?;
        mask(&token);
        *cache = Some(Minted {
            token: token.clone(),
            at: Instant::now(),
        });
        Ok(token)
    }

    /// Forget the cached token, so the next [`Self::token`] mints a new one.
    ///
    /// For when GitHub rejects a token we believed was still good.
    pub fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn mint(&self) -> Result<String, GhError> {
        let (Some(client_id), Some(key)) = (&self.client_id, &self.private_key) else {
            return Err(app_error(
                "SHIP_APP_CLIENT_ID and SHIP_APP_PRIVATE_KEY must be set together",
                Some("set both, or neither to let `gh` authenticate".into()),
            ));
        };

        let jwt = sign_jwt(client_id, key, unix_now())?;

        let installation = match &self.installation_id {
            Some(id) => id.clone(),
            None => self.lookup_installation(&jwt)?,
        };

        let path = format!("app/installations/{installation}/access_tokens");
        let mut args = vec!["-X".to_string(), "POST".into(), path];
        if let Some((_, name)) = self.repo.as_deref().and_then(split_repo) {
            // Scope the token to the one repository gh-ship operates on, as
            // `actions/create-github-app-token` does by default.
            args.push("-f".into());
            args.push(format!("repositories[]={name}"));
        }

        #[derive(Deserialize)]
        struct Response {
            token: String,
        }
        let out = api(&jwt, &args)?;
        serde_json::from_str::<Response>(&out)
            .map(|r| r.token)
            .map_err(|e| {
                app_error(
                    &format!("GitHub's access-token response was not understood: {e}"),
                    None,
                )
            })
    }

    fn lookup_installation(&self, jwt: &str) -> Result<String, GhError> {
        let Some((owner, name)) = self.repo.as_deref().and_then(split_repo) else {
            return Err(app_error(
                "cannot tell which repository's GitHub App installation to use",
                Some(
                    "pass `--repo OWNER/REPO` (or set SHIP_REPO / GITHUB_REPOSITORY), \
                     or set SHIP_APP_INSTALLATION_ID"
                        .into(),
                ),
            ));
        };

        #[derive(Deserialize)]
        struct Installation {
            id: u64,
        }
        let out = api(jwt, &[format!("repos/{owner}/{name}/installation")])?;
        serde_json::from_str::<Installation>(&out)
            .map(|i| i.id.to_string())
            .map_err(|e| {
                app_error(
                    &format!("GitHub's installation response was not understood: {e}"),
                    None,
                )
            })
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Split `[HOST/]OWNER/REPO` into its last two segments.
fn split_repo(repo: &str) -> Option<(&str, &str)> {
    let mut parts = repo.trim_end_matches('/').rsplit('/');
    let name = parts.next().filter(|s| !s.is_empty())?;
    let owner = parts.next().filter(|s| !s.is_empty())?;
    Some((owner, name))
}

/// Ask the runner to redact a freshly minted token from the logs.
///
/// On stderr, because stdout carries the machine-readable output of some
/// commands. Without this, `actions/create-github-app-token`'s masking would
/// be lost along with the action.
fn mask(token: &str) {
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        eprintln!("::add-mask::{token}");
    }
}

fn app_error(message: &str, help: Option<String>) -> GhError {
    GhError::AppAuth {
        message: message.to_string(),
        help,
    }
}

/// Run `gh api` authenticated as the App itself, with a JWT.
///
/// Not [`super::Gh`]'s `exec`: that injects an installation token, which is
/// exactly what is being fetched. `-H` is what makes this work — `gh` would
/// otherwise send the JWT as `token …`, and GitHub documents `Bearer` for
/// JWTs. The JWT lives nine minutes and can only act as the App, so having
/// it in the argument list is an acceptable cost.
fn api<S: AsRef<std::ffi::OsStr>>(jwt: &str, args: &[S]) -> Result<String, GhError> {
    let mut cmd = Command::new("gh");
    cmd.arg("api")
        .arg("-H")
        .arg(format!("Authorization: Bearer {jwt}"))
        .args(args);
    super::cli::apply_token(&mut cmd, jwt);

    let display = format!(
        "api {}",
        args.iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );

    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GhError::NotFound
        } else {
            app_error(&format!("could not run `gh {display}`: {e}"), None)
        }
    })?;

    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|e| app_error(&format!("`gh {display}` printed invalid UTF-8: {e}"), None));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(explain(&display, &stderr))
}

/// Turn a failed JWT-authenticated call into advice about the likely cause.
fn explain(display: &str, stderr: &str) -> GhError {
    let lower = stderr.to_lowercase();

    let help = if lower.contains("401") || lower.contains("bad credentials") {
        "GitHub rejected the App's JWT: check that SHIP_APP_CLIENT_ID is the App's client ID \
         (or its numeric App ID) and that SHIP_APP_PRIVATE_KEY is a private key generated for \
         that App. A skewed runner clock also causes this."
    } else if lower.contains("404") {
        "the App is not installed on this repository, or the installation does not exist. \
         Install it on the repository, or check SHIP_APP_INSTALLATION_ID."
    } else if lower.contains("422") {
        "the App's installation cannot access this repository, or the repository is owned by \
         a different account than the installation."
    } else {
        ""
    };

    app_error(
        &format!("`gh {display}` failed: {stderr}"),
        (!help.is_empty()).then(|| help.to_string()),
    )
}

#[derive(serde::Serialize)]
struct Claims<'a> {
    iat: u64,
    exp: u64,
    iss: &'a str,
}

/// Build and sign the RS256 JWT that authenticates as the App.
pub(crate) fn sign_jwt(client_id: &str, private_key: &str, now: u64) -> Result<String, GhError> {
    let key = parse_key(private_key)?;

    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::to_vec(&Claims {
        iat: now.saturating_sub(JWT_BACKDATE),
        exp: now + JWT_LIFETIME,
        iss: client_id,
    })
    .map_err(|e| app_error(&format!("could not encode the JWT claims: {e}"), None))?;
    let message = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims));

    let signature = SigningKey::<Sha256>::new(key).sign(message.as_bytes());
    Ok(format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

/// Parse a PEM private key, as GitHub issues it (PKCS#1) or as PKCS#8.
///
/// Secrets stored on one line often carry a literal `\n` where a newline
/// belongs, so that is accepted too.
fn parse_key(pem: &str) -> Result<RsaPrivateKey, GhError> {
    let pem = if pem.contains('\n') {
        pem.trim().to_string()
    } else {
        pem.trim().replace("\\n", "\n")
    };

    RsaPrivateKey::from_pkcs1_pem(&pem)
        .or_else(|_| RsaPrivateKey::from_pkcs8_pem(&pem))
        .map_err(|_| {
            app_error(
                "SHIP_APP_PRIVATE_KEY is not a valid RSA private key in PEM format",
                Some(
                    "set it to the full contents of the .pem file GitHub gave you, \
                     including the BEGIN and END lines"
                        .into(),
                ),
            )
        })
}

#[cfg(test)]
mod tests {
    use rsa::RsaPublicKey;
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::signature::Verifier;

    use super::*;

    const PKCS1: &str = include_str!("../../tests/fixtures/app-key.pem");
    const PKCS8: &str = include_str!("../../tests/fixtures/app-key-pkcs8.pem");

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn decode(part: &str) -> serde_json::Value {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
    }

    #[test]
    fn jwt_has_rs256_header_and_app_claims() {
        let jwt = sign_jwt("Iv1.abc", PKCS1, 1_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        assert_eq!(
            decode(parts[0]),
            serde_json::json!({"alg":"RS256","typ":"JWT"})
        );
        assert_eq!(
            decode(parts[1]),
            serde_json::json!({"iat": 1_000_000 - 60, "exp": 1_000_000 + 540, "iss": "Iv1.abc"})
        );
    }

    #[test]
    fn jwt_signature_verifies_against_the_public_key() {
        let jwt = sign_jwt("Iv1.abc", PKCS1, 1_000_000).unwrap();
        let (message, signature) = jwt.rsplit_once('.').unwrap();

        let public = RsaPublicKey::from(&parse_key(PKCS1).unwrap());
        let signature =
            Signature::try_from(URL_SAFE_NO_PAD.decode(signature).unwrap().as_slice()).unwrap();
        VerifyingKey::<Sha256>::new(public)
            .verify(message.as_bytes(), &signature)
            .expect("signature must verify");
    }

    #[test]
    fn jwt_signing_is_deterministic_for_a_given_instant() {
        assert_eq!(
            sign_jwt("x", PKCS1, 5_000).unwrap(),
            sign_jwt("x", PKCS1, 5_000).unwrap()
        );
    }

    #[test]
    fn both_pem_encodings_parse() {
        assert!(parse_key(PKCS1).is_ok());
        assert!(parse_key(PKCS8).is_ok());
    }

    #[test]
    fn a_key_squashed_onto_one_line_parses() {
        let one_line = PKCS1.trim().replace('\n', "\\n");
        assert!(!one_line.contains('\n'));
        assert!(parse_key(&one_line).is_ok());
    }

    #[test]
    fn garbage_is_not_a_key() {
        let err = sign_jwt("x", "not a key", 0).unwrap_err();
        assert!(matches!(err, GhError::AppAuth { .. }));
        assert!(err.to_string().contains("SHIP_APP_PRIVATE_KEY"));
    }

    #[test]
    fn a_token_is_replaced_before_it_expires() {
        let hour = secs(3600);
        assert!(!refresh_due(secs(0), hour));
        assert!(!refresh_due(secs(54 * 60), hour));
        assert!(refresh_due(secs(55 * 60), hour));
        assert!(refresh_due(secs(59 * 60), hour));
        assert!(refresh_due(hour, hour));
    }

    #[test]
    fn a_short_lifetime_keeps_most_of_its_length() {
        // The 5 minute margin would swallow a 20 second lifetime whole.
        assert!(!refresh_due(secs(14), secs(20)));
        assert!(refresh_due(secs(15), secs(20)));
        assert!(refresh_due(secs(0), secs(0)));
    }

    #[test]
    fn half_configured_credentials_name_the_problem() {
        let missing_key = AppAuth::new(Some("Iv1.abc"), None, None, Some("o/r"));
        let err = missing_key.token().unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");

        let missing_id = AppAuth::new(None, Some(PKCS1), None, Some("o/r"));
        assert!(missing_id.token().is_err());
    }

    #[test]
    fn an_unknown_repository_needs_an_installation_id() {
        let app = AppAuth::new(Some("Iv1.abc"), Some(PKCS1), None, None);
        let err = app.token().unwrap_err();
        assert!(err.to_string().contains("which repository"), "{err}");
    }

    #[test]
    fn repositories_are_split_from_their_host() {
        assert_eq!(split_repo("o/r"), Some(("o", "r")));
        assert_eq!(split_repo("github.example.com/o/r"), Some(("o", "r")));
        assert_eq!(split_repo("o/r/"), Some(("o", "r")));
        assert_eq!(split_repo("r"), None);
        assert_eq!(split_repo(""), None);
    }

    #[test]
    fn debug_output_hides_the_key() {
        let app = AppAuth::new(Some("Iv1.abc"), Some(PKCS1), None, None);
        let shown = format!("{app:?}");
        assert!(shown.contains("Iv1.abc"));
        assert!(!shown.contains("PRIVATE KEY"));
    }
}
