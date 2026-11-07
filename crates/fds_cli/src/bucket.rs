use super::output;
use super::util::{print_json, read_json, read_json_redacted, AclMode, CannedAcl};
use anyhow::Result;
use clap::Subcommand;
use fds::{bucket::*, object::*, *};
#[derive(Subcommand)]
pub enum BucketCmd {
    Create {
        name: String,
        /// Passed verbatim to the SDK (e.g. STANDARD, STANDARD_IA, Archive)
        #[arg(long)]
        storage_class: Option<String>,
        #[arg(long)]
        org_id: Option<String>,
        #[arg(long, value_enum)]
        acl: Vec<CannedAcl>,
    },
    Exists {
        name: String,
    },
    Delete {
        name: String,
    },
    Info {
        name: String,
    },
    List,
    ListAuthorized,
    /// Requires --oauth; lists buckets bound to the exchanged app token
    ListApp,
    SetAcl {
        name: String,
        #[arg(long)]
        file: String,
        #[arg(long, value_enum, default_value = "add")]
        action: AclMode,
    },
    Migrate {
        name: String,
        #[arg(long)]
        org_id: String,
        #[arg(long)]
        team_id: String,
    },
    GetAcl {
        name: String,
    },
    GetLifecycle {
        name: String,
        #[arg(long)]
        rule_id: Option<String>,
    },
    SetLifecycle {
        name: String,
        /// Path to a JSON file with a LifecycleConfig
        #[arg(long)]
        file: String,
    },
    SetLifecycleRule {
        name: String,
        /// Path to a JSON file with a LifecycleRule
        #[arg(long)]
        file: String,
    },
    GetAccessLog {
        name: String,
    },
    SetAccessLog {
        name: String,
        /// Full AccessLog JSON, including enabled=false to disable
        #[arg(long)]
        file: String,
    },
}

pub async fn run_bucket(client: &Client, cmd: &BucketCmd) -> Result<()> {
    match cmd {
        BucketCmd::Create {
            name,
            storage_class,
            org_id,
            acl,
        } => {
            let mut request = client.create_bucket(name).acl(acl.iter().map(|v| v.sdk()));
            if let Some(value) = storage_class {
                request = request.storage_class(StorageClass::from(value.as_str()));
            }
            if let Some(value) = org_id {
                request = request.org_id(value);
            }
            request.send().await?;
            output::ok()?;
        }
        BucketCmd::Exists { name } => println!("{}", client.bucket_exists(name).send().await?),
        BucketCmd::Delete { name } => {
            client.delete_bucket(name).send().await?;
            output::ok()?;
        }
        BucketCmd::Info { name } => {
            print_json(&output::bucket(&client.get_bucket_info(name).send().await?))?
        }
        BucketCmd::List => print_json(&output::buckets(&client.list_buckets().send().await?))?,
        BucketCmd::ListApp => print_json(&output::app_buckets(
            &client.list_app_buckets().send().await?,
        ))?,
        BucketCmd::SetAcl { name, file, action } => {
            client
                .set_bucket_acl(name, read_json(file)?)
                .action(action.sdk())
                .send()
                .await?;
            output::ok()?;
        }
        BucketCmd::ListAuthorized => print_json(&output::buckets(
            &client.list_authorized_buckets().send().await?,
        ))?,
        BucketCmd::Migrate {
            name,
            org_id,
            team_id,
        } => {
            client.migrate_bucket(name, org_id, team_id).send().await?;
            output::ok()?;
        }
        BucketCmd::GetAcl { name } => print_json(&client.get_bucket_acl(name).send().await?)?,
        BucketCmd::GetLifecycle { name, rule_id } => {
            let mut request = client.get_lifecycle_config(name);
            if let Some(rule_id) = rule_id {
                request = request.rule_id(rule_id);
            }
            let cfg = request.send().await?;
            print_json(&cfg)?;
        }
        BucketCmd::SetLifecycle { name, file } => {
            let cfg: LifecycleConfig = read_json_redacted(file)?;
            client.set_lifecycle_config(name, cfg).send().await?;
            output::ok()?;
        }
        BucketCmd::SetLifecycleRule { name, file } => {
            let rule: LifecycleRule = read_json_redacted(file)?;
            client.set_lifecycle_rule(name, rule).send().await?;
            output::ok()?;
        }
        BucketCmd::GetAccessLog { name } => print_json(&client.get_access_log(name).send().await?)?,
        BucketCmd::SetAccessLog { name, file } => {
            let mut config: AccessLog = read_json(file)?;
            config.bucket_name = name.clone();
            client.set_access_log(name, config).send().await?;
            output::ok()?;
        }
    }
    Ok(())
}
