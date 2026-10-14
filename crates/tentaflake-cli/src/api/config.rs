use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Config {
    schema_version: u8,
    pub host: String,
    pub origin: String,
    pub listen: SocketAddr,
    pub principal: Principal,
    pub operator_login: bool,
    pub cli_bearer: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Principal {
    pub id: String,
    pub name: String,
}

pub(super) fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::parse(&crate::admission_host::read_store_document(
            path,
            16 * 1024,
        )?)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| "invalid host API configuration")?;
        if value.schema_version != 1
            || !id(&value.host)
            || !id(&value.principal.id)
            || value.principal.name.is_empty()
            || value.principal.name.chars().count() > 160
            || value.principal.name.chars().any(char::is_control)
            || !value.listen.ip().is_loopback()
            || value.listen.port() == 0
            || (!value.operator_login && !value.cli_bearer)
        {
            return Err(
                "invalid host API identity, loopback listener or authentication settings".into(),
            );
        }
        Ok(value)
    }

    pub fn auth_settings(&self) -> Result<super::session::AuthSettings, String> {
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("host API requires systemd runtime credentials")?;
        Ok(super::session::AuthSettings {
            origin: self.origin.clone(),
            operator_credential: self
                .operator_login
                .then(|| directory.join("operator-token")),
            bearer_credential: self.cli_bearer.then(|| directory.join("cli-bearer")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_private_listener_and_explicit_identity_are_accepted() {
        let valid = json!({"schemaVersion":1,"host":"host-fixture","origin":"https://host.example.invalid",
            "listen":"127.0.0.1:8181","principal":{"id":"operator","name":"Operator"},
            "operatorLogin":true,"cliBearer":false});
        assert!(Config::parse(&serde_json::to_vec(&valid).unwrap()).is_ok());
        for (field, bad) in [
            ("host", json!("private:host")),
            ("listen", json!("0.0.0.0:8181")),
            ("listen", json!("[::]:8181")),
            ("listen", json!("127.0.0.1:0")),
            ("schemaVersion", json!(2)),
            ("operatorLogin", json!(false)),
            ("secret", json!("not-a-configuration-field")),
        ] {
            let mut value = valid.clone();
            value[field] = bad;
            assert!(
                Config::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
    }
}
