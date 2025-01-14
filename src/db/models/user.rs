use chrono::{NaiveDateTime, Utc};
use serde_json::Value;

use crate::crypto;
use crate::CONFIG;

#[derive(Debug, Identifiable, Queryable, Insertable)]
#[table_name = "users"]
#[primary_key(uuid)]
pub struct User {
    pub uuid: String,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,

    pub email: String,
    pub name: String,

    pub password_hash: Vec<u8>,
    pub salt: Vec<u8>,
    pub password_iterations: i32,
    pub password_hint: Option<String>,

    pub akey: String,
    pub private_key: Option<String>,
    pub public_key: Option<String>,

    #[column_name = "totp_secret"]
    _totp_secret: Option<String>,
    pub totp_recover: Option<String>,

    pub security_stamp: String,

    pub equivalent_domains: String,
    pub excluded_globals: String,

    pub client_kdf_type: i32,
    pub client_kdf_iter: i32,
}

enum UserStatus {
    Enabled = 0,
    Invited = 1,
    _Disabled = 2,
}

/// Local methods
impl User {
    pub const CLIENT_KDF_TYPE_DEFAULT: i32 = 0; // PBKDF2: 0
    pub const CLIENT_KDF_ITER_DEFAULT: i32 = 100_000;

    pub fn new(mail: String) -> Self {
        let now = Utc::now().naive_utc();
        let email = mail.to_lowercase();

        Self {
            uuid: crate::util::get_uuid(),
            created_at: now,
            updated_at: now,
            name: email.clone(),
            email,
            akey: String::new(),

            password_hash: Vec::new(),
            salt: crypto::get_random_64(),
            password_iterations: CONFIG.password_iterations(),

            security_stamp: crate::util::get_uuid(),

            password_hint: None,
            private_key: None,
            public_key: None,

            _totp_secret: None,
            totp_recover: None,

            equivalent_domains: "[]".to_string(),
            excluded_globals: "[]".to_string(),

            client_kdf_type: Self::CLIENT_KDF_TYPE_DEFAULT,
            client_kdf_iter: Self::CLIENT_KDF_ITER_DEFAULT,
        }
    }

    pub fn check_valid_password(&self, password: &str) -> bool {
        crypto::verify_password_hash(
            password.as_bytes(),
            &self.salt,
            &self.password_hash,
            self.password_iterations as u32,
        )
    }

    pub fn check_valid_recovery_code(&self, recovery_code: &str) -> bool {
        if let Some(ref totp_recover) = self.totp_recover {
            crate::crypto::ct_eq(recovery_code, totp_recover.to_lowercase())
        } else {
            false
        }
    }

    pub fn set_password(&mut self, password: &str) {
        self.password_hash = crypto::hash_password(password.as_bytes(), &self.salt, self.password_iterations as u32);
    }

    pub fn reset_security_stamp(&mut self) {
        self.security_stamp = crate::util::get_uuid();
    }
}

use super::{Cipher, Device, Folder, TwoFactor, UserOrgType, UserOrganization};
use crate::db::schema::{invitations, users};
use crate::db::DbConn;
use diesel;
use diesel::prelude::*;

use crate::api::EmptyResult;
use crate::error::MapResult;

/// Database methods
impl User {
    pub fn to_json(&self, conn: &DbConn) -> Value {
        let orgs = UserOrganization::find_by_user(&self.uuid, conn);
        let orgs_json: Vec<Value> = orgs.iter().map(|c| c.to_json(&conn)).collect();
        let twofactor_enabled = !TwoFactor::find_by_user(&self.uuid, conn).is_empty();

        // TODO: Might want to save the status field in the DB
        let status = if self.password_hash.is_empty() {
            UserStatus::Invited
        } else {
            UserStatus::Enabled
        };

        json!({
            "_Status": status as i32,
            "Id": self.uuid,
            "Name": self.name,
            "Email": self.email,
            "EmailVerified": true,
            "Premium": true,
            "MasterPasswordHint": self.password_hint,
            "Culture": "en-US",
            "TwoFactorEnabled": twofactor_enabled,
            "Key": self.akey,
            "PrivateKey": self.private_key,
            "SecurityStamp": self.security_stamp,
            "Organizations": orgs_json,
            "Object": "profile"
        })
    }

    pub fn save(&mut self, conn: &DbConn) -> EmptyResult {
        if self.email.trim().is_empty() {
            err!("User email can't be empty")
        }

        self.updated_at = Utc::now().naive_utc();

        diesel::replace_into(users::table) // Insert or update
            .values(&*self)
            .execute(&**conn)
            .map_res("Error saving user")
    }

    pub fn delete(self, conn: &DbConn) -> EmptyResult {
        for user_org in UserOrganization::find_by_user(&self.uuid, &*conn) {
            if user_org.atype == UserOrgType::Owner {
                let owner_type = UserOrgType::Owner as i32;
                if UserOrganization::find_by_org_and_type(&user_org.org_uuid, owner_type, &conn).len() <= 1 {
                    err!("Can't delete last owner")
                }
            }
        }

        UserOrganization::delete_all_by_user(&self.uuid, &*conn)?;
        Cipher::delete_all_by_user(&self.uuid, &*conn)?;
        Folder::delete_all_by_user(&self.uuid, &*conn)?;
        Device::delete_all_by_user(&self.uuid, &*conn)?;
        TwoFactor::delete_all_by_user(&self.uuid, &*conn)?;
        Invitation::take(&self.email, &*conn); // Delete invitation if any

        diesel::delete(users::table.filter(users::uuid.eq(self.uuid)))
            .execute(&**conn)
            .map_res("Error deleting user")
    }

    pub fn update_uuid_revision(uuid: &str, conn: &DbConn) {
        if let Err(e) = Self::_update_revision(uuid, &Utc::now().naive_utc(), conn) {
            warn!("Failed to update revision for {}: {:#?}", uuid, e);
        }
    }

    pub fn update_all_revisions(conn: &DbConn) -> EmptyResult {
        let updated_at = Utc::now().naive_utc();

        crate::util::retry(
            || {
                diesel::update(users::table)
                    .set(users::updated_at.eq(updated_at))
                    .execute(&**conn)
            },
            10,
        )
        .map_res("Error updating revision date for all users")
    }

    pub fn update_revision(&mut self, conn: &DbConn) -> EmptyResult {
        self.updated_at = Utc::now().naive_utc();

        Self::_update_revision(&self.uuid, &self.updated_at, conn)
    }

    fn _update_revision(uuid: &str, date: &NaiveDateTime, conn: &DbConn) -> EmptyResult {
        crate::util::retry(
            || {
                diesel::update(users::table.filter(users::uuid.eq(uuid)))
                    .set(users::updated_at.eq(date))
                    .execute(&**conn)
            },
            10,
        )
        .map_res("Error updating user revision")
    }

    pub fn find_by_mail(mail: &str, conn: &DbConn) -> Option<Self> {
        let lower_mail = mail.to_lowercase();
        users::table
            .filter(users::email.eq(lower_mail))
            .first::<Self>(&**conn)
            .ok()
    }

    pub fn find_by_uuid(uuid: &str, conn: &DbConn) -> Option<Self> {
        users::table.filter(users::uuid.eq(uuid)).first::<Self>(&**conn).ok()
    }

    pub fn get_all(conn: &DbConn) -> Vec<Self> {
        users::table.load::<Self>(&**conn).expect("Error loading users")
    }
}

#[derive(Debug, Identifiable, Queryable, Insertable)]
#[table_name = "invitations"]
#[primary_key(email)]
pub struct Invitation {
    pub email: String,
}

impl Invitation {
    pub fn new(email: String) -> Self {
        Self { email }
    }

    pub fn save(&self, conn: &DbConn) -> EmptyResult {
        if self.email.trim().is_empty() {
            err!("Invitation email can't be empty")
        }

        diesel::replace_into(invitations::table)
            .values(self)
            .execute(&**conn)
            .map_res("Error saving invitation")
    }

    pub fn delete(self, conn: &DbConn) -> EmptyResult {
        diesel::delete(invitations::table.filter(invitations::email.eq(self.email)))
            .execute(&**conn)
            .map_res("Error deleting invitation")
    }

    pub fn find_by_mail(mail: &str, conn: &DbConn) -> Option<Self> {
        let lower_mail = mail.to_lowercase();
        invitations::table
            .filter(invitations::email.eq(lower_mail))
            .first::<Self>(&**conn)
            .ok()
    }

    pub fn take(mail: &str, conn: &DbConn) -> bool {
        CONFIG.invitations_allowed()
            && match Self::find_by_mail(mail, &conn) {
                Some(invitation) => invitation.delete(&conn).is_ok(),
                None => false,
            }
    }
}
