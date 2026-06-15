use std::collections::HashMap;
use std::env;
use crate::config::Secret;

pub fn collect_secrets_env(
    secrets: &[Secret],
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = HashMap::new();

    for secret in secrets {
        let value = env::var(&secret.env_var).map_err(|_| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Secret '{}' is configured to come from env var '{}', but it is not set\n\
                     Hint: export {}=<value> before running forge",
                    secret.name, secret.env_var, secret.env_var
                ),
            ))
        })?;

        out.insert(secret.name.clone(), value);
    }

    Ok(out)
}
