//! `data-mesh` — the workspace's single binding to the data-mesh standard
//! offering.
//!
//! Services depend on **this** crate, so the offering's coordinate lives in one
//! place. The HTTP binding itself is [`mesh`] — an in-workspace call to the
//! deployed data-mesh Service, **not** a git or registry dependency. It is the **only
//! persistence path** for apps-middle-ware services — there is no direct
//! `surrealdb` dependency anywhere in the workspace (data-mesh-consumption
//! policy; the `offering-facade` canonical pattern: bind the facade, never the
//! source store).
//!
//! ```no_run
//! # async fn demo() -> Result<(), data_mesh::Error> {
//! #[derive(serde::Serialize)]
//! struct Account { id: String, balance: i64 }
//! let account = Account { id: "acc-1".into(), balance: 42 };
//! let client = data_mesh::from_env()?;
//! let created = client
//!     .create_document("accounts", &data_mesh::to_document(&account)?)
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!

// No PROD_URL. It was removed upstream on 2026-08-02 (data-mesh#133, "drop dead
// dfc.aivcs.io PROD_URL") and re-exporting it here does not compile against any
// pin at or after that commit — including the one this crate uses.
//
// It should not come back. data-mesh is a cluster-only system of record and is
// never internet-exposed, so a public PROD_URL is a contradiction, not a missing
// convenience. Off-cluster callers go through the authenticated edge gateway,
// not a direct URL to the mesh.
mod mesh;

pub use mesh::{
    Client, ClientConfig, Created, Error, Result, TransactionOp, TransactionOpEntry,
    TransactionResult, IN_CLUSTER_URL,
};

use serde::{de::DeserializeOwned, Serialize};

/// Bind a data-mesh [`Client`] from the environment (`DATA_FABRIC_URL`,
/// `DATA_MESH_TENANT_ID`, optional CF-Access / bearer creds) — the entry point a
/// service calls at startup. The deployment, not the code, chooses the endpoint
/// and tenant.
pub fn from_env() -> Result<Client> {
    Client::from_env()
}

/// Serialize a typed domain value into the opaque JSON document payload the
/// offering stores. Pairs with [`from_document`].
pub fn to_document<T: Serialize>(value: &T) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(value)
}

/// Deserialize a document payload (as returned by `get_document` /
/// `list_documents` / `query_documents`) back into a typed domain value.
pub fn from_document<T: DeserializeOwned>(doc: serde_json::Value) -> serde_json::Result<T> {
    serde_json::from_value(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Account {
        id: String,
        balance: i64,
    }

    #[test]
    fn document_round_trips_a_typed_value() {
        let account = Account {
            id: "acc-1".into(),
            balance: 42,
        };
        let doc = to_document(&account).unwrap();
        assert_eq!(doc["id"], "acc-1");
        assert_eq!(doc["balance"], 42);
        let back: Account = from_document(doc).unwrap();
        assert_eq!(back, account);
    }

    #[test]
    fn client_binds_to_an_explicit_endpoint_without_env() {
        // `ClientConfig`'s fields are public, so a service (or test) can bind to
        // an explicit endpoint/tenant without touching process env.
        let client = Client::new(ClientConfig {
            base_url: "http://data-mesh.test".into(),
            tenant_id: "tenant-x".into(),
            tenant_role: "builder".into(),
            cf_client_id: None,
            cf_client_secret: None,
            bearer_token: None,
        });
        assert_eq!(client.base_url(), "http://data-mesh.test");
        assert_eq!(client.tenant_id(), "tenant-x");
    }
}
