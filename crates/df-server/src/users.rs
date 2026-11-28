//! User & session management CLI subcommands (`df-server users ...`).

use std::str::FromStr;

use df_auth::{Role, hash_password, hash_pin};
use uuid::Uuid;

use crate::db::{open_migrated_pool, print_json};

/// Insert the org row when it does not exist yet (bootstrap case:
/// the first admin on a fresh database). The name is only a label;
/// pass `--org-name` to set it.
pub(crate) async fn ensure_org(pool: &df_db::AnyPool, org_id: Uuid, org_name: Option<String>) {
    if df_db::get_organization(pool, org_id).await.is_ok() {
        return;
    }
    let org = df_db::Organization {
        id: org_id,
        name: org_name.unwrap_or_else(|| "Decor POS".to_string()),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({}),
        settings_json: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    if let Err(err) = df_db::insert_organization(pool, &org).await {
        eprintln!("df-server users: cannot create organization {org_id}: {err}");
        std::process::exit(1);
    }
}

/// Read a password: the env var first (read once, never logged),
/// otherwise a stdin prompt. Returns `None` when the prompt gets an
/// empty line — the caller decides whether that is an error (a
/// password is required) or fine (a PIN-only user).
pub(crate) fn read_password(password_env: &str) -> Option<String> {
    if let Ok(password) = std::env::var(password_env)
        && !password.is_empty()
    {
        return Some(password);
    }
    eprint!("Password (min 8 characters): ");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => {
            let password = line.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                None
            } else {
                Some(password)
            }
        }
        Err(err) => {
            eprintln!("df-server users: cannot read password from stdin: {err}");
            std::process::exit(2);
        }
    }
}

/// Ask the operator to type `YES` (exactly) to confirm a dangerous
/// step. Anything else aborts.
pub(crate) fn confirm_yes(prompt: &str) -> bool {
    eprint!("{prompt}");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => line.trim() == "YES",
        Err(_) => false,
    }
}

pub(crate) fn hash_or_exit(password: &str, what: &str) -> String {
    match hash_password(password) {
        Ok(hash) => hash,
        Err(err) => {
            eprintln!("df-server users: {what}: {err}");
            std::process::exit(2);
        }
    }
}

pub(crate) async fn run_users_create_admin(
    db: String,
    org_id: Uuid,
    org_name: Option<String>,
    email: String,
    display_name: String,
    force: bool,
    password_env: String,
) -> ! {
    let pool = open_migrated_pool(&db).await;
    ensure_org(&pool, org_id, org_name).await;
    let existing = match df_db::count_users_with_role(&pool, org_id, Role::Admin).await {
        Ok(n) => n,
        Err(err) => {
            eprintln!("df-server users create-admin: cannot count admins: {err}");
            std::process::exit(1);
        }
    };
    if existing > 0 && !force {
        eprintln!(
            "df-server users create-admin: org {org_id} already has {existing} admin(s) — pass --force and confirm with YES to add another"
        );
        std::process::exit(2);
    }
    if existing > 0
        && !confirm_yes(&format!(
            "org {org_id} already has {existing} admin(s); type YES to create another admin: "
        ))
    {
        eprintln!("df-server users create-admin: aborted");
        std::process::exit(2);
    }
    let Some(password) = read_password(&password_env) else {
        eprintln!(
            "df-server users create-admin: empty password — set the env var or type a password"
        );
        std::process::exit(2);
    };
    let hash = hash_or_exit(&password, "invalid password");
    let user = match df_db::create_user(
        &pool,
        &df_db::NewUser {
            org_id,
            email: email.clone(),
            display_name,
            password_hash: Some(hash),
            counter_pin_hash: None,
            badge_token_hash: None,
            role: Role::Admin,
            portal_customer_id: None,
            active: true,
        },
    )
    .await
    {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users create-admin: cannot create admin {email}: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name,
        "role": "Admin",
        "org_id": org_id,
    }));
}

/// Arguments for `df-server users create`, bundled so the
/// function stays under clippy's too-many-arguments lint.
pub(crate) struct CreateUserArgs {
    pub(crate) db: String,
    pub(crate) org_id: Uuid,
    pub(crate) org_name: Option<String>,
    pub(crate) email: String,
    pub(crate) display_name: String,
    pub(crate) role_text: String,
    pub(crate) pin: Option<String>,
    pub(crate) customer_id: Option<Uuid>,
    pub(crate) no_password: bool,
    pub(crate) password_env: String,
}

pub(crate) async fn run_users_create(args: CreateUserArgs) -> ! {
    let CreateUserArgs {
        db,
        org_id,
        org_name,
        email,
        display_name,
        role_text,
        pin,
        customer_id,
        no_password,
        password_env,
    } = args;
    let role = match Role::from_str(&role_text) {
        Ok(role) => role,
        Err(err) => {
            eprintln!("df-server users create: {err}");
            std::process::exit(2);
        }
    };
    if role == Role::PortalContractor && customer_id.is_none() {
        eprintln!(
            "df-server users create: --customer-id is required for --role PortalContractor (the customer the portal user may access)"
        );
        std::process::exit(2);
    }
    if role != Role::PortalContractor && customer_id.is_some() {
        eprintln!("df-server users create: --customer-id is only for --role PortalContractor");
        std::process::exit(2);
    }
    let pool = open_migrated_pool(&db).await;
    ensure_org(&pool, org_id, org_name).await;
    if let Some(customer) = customer_id
        && let Err(err) = df_db::get_customer(&pool, customer).await
    {
        eprintln!("df-server users create: cannot read customer {customer}: {err}");
        std::process::exit(1);
    }
    let pin_hash = match pin {
        Some(pin) => match hash_pin(&pin) {
            Ok(hash) => Some(hash),
            Err(err) => {
                eprintln!("df-server users create: invalid --pin: {err}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let password_hash = if no_password {
        None
    } else {
        match read_password(&password_env) {
            Some(password) => Some(hash_or_exit(&password, "invalid password")),
            None => {
                if pin_hash.is_none() {
                    eprintln!(
                        "df-server users create: empty password — set the env var, type a password, pass --pin, or use --no-password for a PIN-only user"
                    );
                    std::process::exit(2);
                }
                None
            }
        }
    };
    if password_hash.is_none() && pin_hash.is_none() {
        eprintln!(
            "df-server users create: a user needs a password or a --pin (pass --no-password with --pin for a PIN-only user)"
        );
        std::process::exit(2);
    }
    let user = match df_db::create_user(
        &pool,
        &df_db::NewUser {
            org_id,
            email: email.clone(),
            display_name,
            password_hash,
            counter_pin_hash: pin_hash,
            badge_token_hash: None,
            role,
            portal_customer_id: customer_id,
            active: true,
        },
    )
    .await
    {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users create: cannot create user {email}: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name,
        "role": role.as_str(),
        "org_id": org_id,
        "portal_customer_id": customer_id,
    }));
}

pub(crate) async fn run_users_list(db: String, org_id: Uuid) -> ! {
    let pool = open_migrated_pool(&db).await;
    let users = match df_db::list_users_for_org(&pool, org_id).await {
        Ok(users) => users,
        Err(err) => {
            eprintln!("df-server users list: cannot list users for org {org_id}: {err}");
            std::process::exit(1);
        }
    };
    let value: Vec<serde_json::Value> = users
        .iter()
        .map(|user| {
            serde_json::json!({
                "id": user.id,
                "email": user.email,
                "display_name": user.display_name,
                "role": df_auth::role_from_id(user.role_id)
                    .map(|role| role.as_str().to_string())
                    .unwrap_or_else(|| format!("unknown({})", user.role_id)),
                "portal_customer_id": user.portal_customer_id,
                "active": user.active,
            })
        })
        .collect();
    print_json(serde_json::Value::Array(value));
}

pub(crate) async fn run_users_revoke_sessions(db: String, email: String) -> ! {
    let pool = open_migrated_pool(&db).await;
    let user = match df_db::get_user_by_email(&pool, &email).await {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users revoke-sessions: cannot find user {email}: {err}");
            std::process::exit(1);
        }
    };
    let revoked = match df_db::revoke_sessions_for_user(&pool, user.id).await {
        Ok(n) => n,
        Err(err) => {
            eprintln!("df-server users revoke-sessions: cannot revoke sessions: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "email": email,
        "user_id": user.id,
        "sessions_revoked": revoked,
    }));
}
