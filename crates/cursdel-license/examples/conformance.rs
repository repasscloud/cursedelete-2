//! Licensing-server conformance checker: validates a licence (and
//! optionally a Business activation certificate) with exactly the code
//! CurseDelete uses, against public keys you supply. Lets a server
//! implementation prove its output is accepted before any application
//! build trusts its keys.
//!
//! ```text
//! cargo run -p cursdel-license --example conformance -- \
//!     --key <keyId>:<algorithm>:<base64url-public-key>   (repeatable) \
//!     --product cursedelete --major 2 \
//!     --license licence.json \
//!     [--activation activation.json --device <deviceId>] \
//!     [--now 2026-10-12T00:00:00Z] \
//!     [--algorithms Ed25519,ECDSA-P256-SHA256,ML-DSA-65]
//! ```
//!
//! Prints a JSON report; exits 0 if everything validated, 1 if not, 2 on
//! a usage error.

use std::process::ExitCode;

use cursdel_license::{
    LicensePolicy, ProductIdentity, SignatureAlgorithm, Term, TrustedKey, TrustedKeySet,
};
use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

struct Args {
    keys: Vec<TrustedKey>,
    product: Option<String>,
    major: Option<u32>,
    license: Option<String>,
    activation: Option<String>,
    device: Option<String>,
    now: OffsetDateTime,
    algorithms: Vec<SignatureAlgorithm>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        keys: Vec::new(),
        product: None,
        major: None,
        license: None,
        activation: None,
        device: None,
        now: OffsetDateTime::now_utc(),
        algorithms: SignatureAlgorithm::ALL.to_vec(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--key" => {
                let v = value()?;
                let mut parts = v.splitn(3, ':');
                let (Some(id), Some(alg), Some(key)) = (parts.next(), parts.next(), parts.next())
                else {
                    return Err("--key must be <keyId>:<algorithm>:<base64url-public-key>".into());
                };
                let alg =
                    SignatureAlgorithm::from_id(alg).ok_or(format!("unknown algorithm '{alg}'"))?;
                args.keys
                    .push(TrustedKey::from_base64url(id, alg, key).map_err(|e| e.to_string())?);
            }
            "--product" => args.product = Some(value()?),
            "--major" => {
                args.major = Some(value()?.parse().map_err(|_| "--major must be a number")?)
            }
            "--license" => args.license = Some(value()?),
            "--activation" => args.activation = Some(value()?),
            "--device" => args.device = Some(value()?),
            "--now" => {
                args.now = OffsetDateTime::parse(&value()?, &Rfc3339)
                    .map_err(|_| "--now must be RFC 3339, e.g. 2026-10-12T00:00:00Z")?
            }
            "--algorithms" => {
                args.algorithms = value()?
                    .split(',')
                    .map(|a| {
                        SignatureAlgorithm::from_id(a.trim())
                            .ok_or(format!("unknown algorithm '{a}'"))
                    })
                    .collect::<Result<_, _>>()?
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if args.activation.is_some() != args.device.is_some() {
        return Err("--activation and --device go together".into());
    }
    Ok(args)
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!(
                "usage: conformance --key <id>:<alg>:<b64url> ... --product <id> --major <n> \
                 --license <file> [--activation <file> --device <id>] [--now <rfc3339>] \
                 [--algorithms a,b]"
            );
            return ExitCode::from(2);
        }
    };
    let (Some(product_id), Some(major_version), Some(license_path)) =
        (args.product, args.major, args.license)
    else {
        eprintln!("error: --product, --major and --license are required");
        return ExitCode::from(2);
    };
    let trusted_keys = match TrustedKeySet::new(args.keys) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let policy = LicensePolicy {
        product: ProductIdentity {
            product_id,
            major_version,
        },
        trusted_keys,
        allowed_algorithms: args.algorithms,
        business_offline_limit: None,
    };

    let mut ok = true;
    let licence = read(&license_path).and_then(|t| {
        policy
            .verify_license(&t, args.now)
            .map_err(|r| format!("{r:?}: {r}"))
    });
    let licence_report = match &licence {
        Ok(l) => {
            let c = &l.claims;
            json!({
                "valid": true,
                "licenseId": c.license_id,
                "edition": c.edition.id(),
                "term": match c.term {
                    Term::Perpetual => "perpetual".to_string(),
                    Term::Subscription { expires_at } => format!(
                        "expires {}",
                        expires_at.format(&Rfc3339).unwrap_or_default()
                    ),
                },
                "signedBy": l.signed_by.iter().map(|s| format!("{} ({})", s.key_id, s.algorithm)).collect::<Vec<_>>(),
            })
        }
        Err(e) => {
            ok = false;
            json!({ "valid": false, "rejection": e })
        }
    };

    let activation_report = match (&args.activation, &args.device, &licence) {
        (Some(path), Some(device), Ok(l)) => {
            match read(path).and_then(|t| {
                policy
                    .verify_activation(&t, &l.claims, device, args.now)
                    .map_err(|r| format!("{r:?}: {r}"))
            }) {
                Ok(a) => json!({ "valid": true, "activationId": a.activation_id }),
                Err(e) => {
                    ok = false;
                    json!({ "valid": false, "rejection": e })
                }
            }
        }
        (Some(_), _, Err(_)) => {
            ok = false;
            json!({ "valid": false, "rejection": "not checked: the licence itself is invalid" })
        }
        _ => serde_json::Value::Null,
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "license": licence_report,
            "activation": activation_report,
        }))
        .unwrap()
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
