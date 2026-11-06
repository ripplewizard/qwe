use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::AttachmentAuthorityState;

pub const DEFAULT_DETACHED_HOLD_TTL_MS: i64 = 5 * 60 * 1_000;
pub const DEFAULT_LEASE_TTL_MS: i64 = 60 * 1_000;
pub const MAX_LEASE_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
pub const DEFAULT_RECLAIM_GRACE_MS: i64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReclaimTokenHash(String);

impl ReclaimTokenHash {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        let digest = Sha256::digest(token.as_bytes());
        let mut encoded = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(encoded, "{byte:02x}");
        }
        Self(encoded)
    }

    #[must_use]
    pub fn from_persisted(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn matches(&self, token: &str) -> bool {
        let candidate = Self::from_token(token);
        constant_time_eq(self.0.as_bytes(), candidate.0.as_bytes())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentOperation {
    ListStatus,
    Attach,
    InputResize,
    ResourcePolicy,
    Detach,
    CloseTaskStop,
    LeaseClaimTakeover,
    LeaseTouchRelease,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AuthorityCredentials<'a> {
    pub owner: Option<&'a str>,
    pub reclaim_token: Option<&'a str>,
    pub lease_generation: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AuthorityError {
    #[error("the session already has an exclusive attachment")]
    AlreadyAttached,
    #[error("the operation requires an active attachment")]
    NotAttached,
    #[error("session owner or fencing generation does not match")]
    NotOwner,
    #[error("the reclaim token is missing or invalid")]
    InvalidReclaimToken,
    #[error("the reclaim grace period has expired")]
    ReclaimExpired,
    #[error("the attachment authority transition is invalid")]
    InvalidTransition,
}

impl AuthorityError {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::AlreadyAttached => "already_attached",
            Self::NotAttached => "not_attached",
            Self::NotOwner => "not_owner",
            Self::InvalidReclaimToken => "invalid_reclaim_token",
            Self::ReclaimExpired => "reclaim_expired",
            Self::InvalidTransition => "invalid_transition",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentAuthority {
    pub state: AttachmentAuthorityState,
    pub owner: Option<String>,
    pub generation: u64,
    pub reclaim_token_hash: Option<ReclaimTokenHash>,
    pub deadline_ms: Option<i64>,
}

impl AttachmentAuthority {
    #[must_use]
    pub const fn unattached() -> Self {
        Self {
            state: AttachmentAuthorityState::Unattached,
            owner: None,
            generation: 0,
            reclaim_token_hash: None,
            deadline_ms: None,
        }
    }

    #[must_use]
    pub fn active(owner: impl Into<String>, token_hash: ReclaimTokenHash) -> Self {
        Self {
            state: AttachmentAuthorityState::Active,
            owner: Some(owner.into()),
            generation: 0,
            reclaim_token_hash: Some(token_hash),
            deadline_ms: None,
        }
    }

    #[must_use]
    pub fn reclaim_grace(
        owner: impl Into<String>,
        generation: u64,
        token_hash: ReclaimTokenHash,
        deadline_ms: i64,
    ) -> Self {
        Self {
            state: AttachmentAuthorityState::ReclaimGrace,
            owner: Some(owner.into()),
            generation,
            reclaim_token_hash: Some(token_hash),
            deadline_ms: Some(deadline_ms),
        }
    }

    #[must_use]
    pub fn held(owner: impl Into<String>, generation: u64, deadline_ms: i64) -> Self {
        Self {
            state: AttachmentAuthorityState::Held,
            owner: Some(owner.into()),
            generation,
            reclaim_token_hash: None,
            deadline_ms: Some(deadline_ms),
        }
    }

    #[must_use]
    pub fn state_at(&self, now_ms: i64) -> AttachmentAuthorityState {
        if matches!(
            self.state,
            AttachmentAuthorityState::ReclaimGrace | AttachmentAuthorityState::Held
        ) && self.deadline_ms.is_some_and(|deadline| deadline <= now_ms)
        {
            AttachmentAuthorityState::Unattached
        } else {
            self.state
        }
    }

    pub fn authorize(
        &self,
        operation: AttachmentOperation,
        credentials: AuthorityCredentials<'_>,
        now_ms: i64,
    ) -> Result<(), AuthorityError> {
        let state = self.state_at(now_ms);
        match operation {
            AttachmentOperation::ListStatus => Ok(()),
            AttachmentOperation::Attach => match state {
                AttachmentAuthorityState::Unattached => Ok(()),
                AttachmentAuthorityState::Active => Err(AuthorityError::AlreadyAttached),
                AttachmentAuthorityState::ReclaimGrace => {
                    let Some(token) = credentials.reclaim_token else {
                        return Err(AuthorityError::InvalidReclaimToken);
                    };
                    let Some(expected) = &self.reclaim_token_hash else {
                        return Err(AuthorityError::InvalidReclaimToken);
                    };
                    expected
                        .matches(token)
                        .then_some(())
                        .ok_or(AuthorityError::InvalidReclaimToken)
                }
                AttachmentAuthorityState::Held => self
                    .owner_matches(credentials.owner)
                    .then_some(())
                    .filter(|()| credentials.lease_generation == Some(self.generation))
                    .ok_or(AuthorityError::NotOwner),
            },
            AttachmentOperation::InputResize | AttachmentOperation::Detach => match state {
                AttachmentAuthorityState::Active => self
                    .owner_matches(credentials.owner)
                    .then_some(())
                    .ok_or(AuthorityError::NotOwner),
                _ => Err(AuthorityError::NotAttached),
            },
            AttachmentOperation::CloseTaskStop | AttachmentOperation::ResourcePolicy => match state
            {
                AttachmentAuthorityState::Active => self
                    .owner_matches(credentials.owner)
                    .then_some(())
                    .ok_or(AuthorityError::NotOwner),
                AttachmentAuthorityState::Held => {
                    self.owner_and_generation_match(credentials.owner, credentials.lease_generation)
                }
                _ => Err(AuthorityError::NotAttached),
            },
            AttachmentOperation::LeaseClaimTakeover => match state {
                AttachmentAuthorityState::Unattached | AttachmentAuthorityState::Held => Ok(()),
                AttachmentAuthorityState::Active => Err(AuthorityError::AlreadyAttached),
                AttachmentAuthorityState::ReclaimGrace => Err(AuthorityError::NotAttached),
            },
            AttachmentOperation::LeaseTouchRelease => match state {
                AttachmentAuthorityState::Held => {
                    self.owner_and_generation_match(credentials.owner, credentials.lease_generation)
                }
                _ => Err(AuthorityError::NotAttached),
            },
        }
    }

    pub fn grant_active(
        &mut self,
        owner: impl Into<String>,
        token_hash: ReclaimTokenHash,
    ) -> Result<(), AuthorityError> {
        if self.state_at(i64::MIN) != AttachmentAuthorityState::Unattached {
            return Err(AuthorityError::InvalidTransition);
        }
        self.state = AttachmentAuthorityState::Active;
        self.owner = Some(owner.into());
        self.reclaim_token_hash = Some(token_hash);
        self.deadline_ms = None;
        Ok(())
    }

    pub fn enter_reclaim_grace(&mut self, deadline_ms: i64) -> Result<(), AuthorityError> {
        if self.state != AttachmentAuthorityState::Active {
            return Err(AuthorityError::InvalidTransition);
        }
        self.state = AttachmentAuthorityState::ReclaimGrace;
        self.deadline_ms = Some(deadline_ms);
        Ok(())
    }

    pub fn attach_from_reclaim(
        &mut self,
        token: &str,
        replacement_token_hash: ReclaimTokenHash,
        now_ms: i64,
    ) -> Result<(), AuthorityError> {
        self.authorize(
            AttachmentOperation::Attach,
            AuthorityCredentials {
                reclaim_token: Some(token),
                ..AuthorityCredentials::default()
            },
            now_ms,
        )?;
        self.state = AttachmentAuthorityState::Active;
        self.reclaim_token_hash = Some(replacement_token_hash);
        self.deadline_ms = None;
        Ok(())
    }

    pub fn attach_from_hold(
        &mut self,
        owner: &str,
        generation: u64,
        replacement_token_hash: ReclaimTokenHash,
        now_ms: i64,
    ) -> Result<(), AuthorityError> {
        self.authorize(
            AttachmentOperation::Attach,
            AuthorityCredentials {
                owner: Some(owner),
                lease_generation: Some(generation),
                ..AuthorityCredentials::default()
            },
            now_ms,
        )?;
        self.state = AttachmentAuthorityState::Active;
        self.owner = Some(owner.into());
        self.generation += 1;
        self.reclaim_token_hash = Some(replacement_token_hash);
        self.deadline_ms = None;
        Ok(())
    }

    pub fn detach(&mut self, hold_ttl_ms: i64, now_ms: i64) -> Result<(), AuthorityError> {
        if self.state != AttachmentAuthorityState::Active {
            return Err(AuthorityError::NotAttached);
        }
        if hold_ttl_ms < 0 {
            return Err(AuthorityError::InvalidTransition);
        }
        self.reclaim_token_hash = None;
        if hold_ttl_ms == 0 {
            self.release();
        } else {
            self.state = AttachmentAuthorityState::Held;
            self.generation += 1;
            self.deadline_ms = Some(now_ms.saturating_add(hold_ttl_ms));
        }
        Ok(())
    }

    pub fn touch_hold(
        &mut self,
        owner: &str,
        generation: u64,
        ttl_ms: i64,
        now_ms: i64,
    ) -> Result<(), AuthorityError> {
        if ttl_ms <= 0 {
            return Err(AuthorityError::InvalidTransition);
        }
        self.authorize(
            AttachmentOperation::LeaseTouchRelease,
            AuthorityCredentials {
                owner: Some(owner),
                lease_generation: Some(generation),
                ..AuthorityCredentials::default()
            },
            now_ms,
        )?;
        self.deadline_ms = Some(now_ms.saturating_add(ttl_ms));
        Ok(())
    }

    pub fn release(&mut self) {
        self.state = AttachmentAuthorityState::Unattached;
        self.owner = None;
        self.reclaim_token_hash = None;
        self.deadline_ms = None;
    }

    fn owner_matches(&self, owner: Option<&str>) -> bool {
        owner.is_some() && self.owner.as_deref() == owner
    }

    fn owner_and_generation_match(
        &self,
        owner: Option<&str>,
        generation: Option<u64>,
    ) -> Result<(), AuthorityError> {
        (self.owner_matches(owner) && generation == Some(self.generation))
            .then_some(())
            .ok_or(AuthorityError::NotOwner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 10_000;

    fn active() -> AttachmentAuthority {
        AttachmentAuthority::active("alice", ReclaimTokenHash::from_token("active-token"))
    }

    fn grace() -> AttachmentAuthority {
        AttachmentAuthority::reclaim_grace(
            "alice",
            2,
            ReclaimTokenHash::from_token("active-token"),
            NOW + 1_000,
        )
    }

    fn held() -> AttachmentAuthority {
        AttachmentAuthority::held("alice", 3, NOW + 1_000)
    }

    fn creds(
        owner: Option<&'static str>,
        token: Option<&'static str>,
        generation: Option<u64>,
    ) -> AuthorityCredentials<'static> {
        AuthorityCredentials {
            owner,
            reclaim_token: token,
            lease_generation: generation,
        }
    }

    #[test]
    fn reclaim_token_hashes_are_deterministic_and_constant_time_compared() {
        let first = ReclaimTokenHash::from_token("secret");
        let second = ReclaimTokenHash::from_token("secret");
        assert_eq!(first, second);
        assert!(first.matches("secret"));
        assert!(!first.matches("Secret"));
        assert_eq!(first.as_str().len(), 64);
    }

    #[test]
    fn authorization_matrix_accepts_and_rejects_every_authority_state() {
        let unattached = AttachmentAuthority::unattached();
        let active = active();
        let grace = grace();
        let held = held();
        let active_owner = creds(Some("alice"), None, None);
        let held_owner = creds(Some("alice"), None, Some(3));
        let no_authority = creds(None, None, None);

        for authority in [&unattached, &active, &grace, &held] {
            assert!(
                authority
                    .authorize(AttachmentOperation::ListStatus, no_authority, NOW)
                    .is_ok()
            );
        }

        assert!(
            unattached
                .authorize(AttachmentOperation::Attach, no_authority, NOW)
                .is_ok()
        );
        assert_eq!(
            active
                .authorize(AttachmentOperation::Attach, no_authority, NOW)
                .unwrap_err(),
            AuthorityError::AlreadyAttached
        );
        assert!(
            grace
                .authorize(
                    AttachmentOperation::Attach,
                    creds(None, Some("active-token"), None),
                    NOW,
                )
                .is_ok()
        );
        assert_eq!(
            grace
                .authorize(
                    AttachmentOperation::Attach,
                    creds(None, Some("stale-token"), None),
                    NOW,
                )
                .unwrap_err(),
            AuthorityError::InvalidReclaimToken
        );
        assert!(
            held.authorize(AttachmentOperation::Attach, held_owner, NOW)
                .is_ok()
        );
        assert_eq!(
            held.authorize(
                AttachmentOperation::Attach,
                creds(Some("alice"), None, Some(2)),
                NOW,
            )
            .unwrap_err(),
            AuthorityError::NotOwner
        );

        for operation in [
            AttachmentOperation::InputResize,
            AttachmentOperation::Detach,
        ] {
            assert_eq!(
                unattached
                    .authorize(operation, no_authority, NOW)
                    .unwrap_err(),
                AuthorityError::NotAttached
            );
            assert!(active.authorize(operation, active_owner, NOW).is_ok());
            assert_eq!(
                grace
                    .authorize(operation, creds(None, Some("active-token"), None), NOW,)
                    .unwrap_err(),
                AuthorityError::NotAttached
            );
            assert_eq!(
                held.authorize(operation, held_owner, NOW).unwrap_err(),
                AuthorityError::NotAttached
            );
        }

        for operation in [
            AttachmentOperation::CloseTaskStop,
            AttachmentOperation::ResourcePolicy,
        ] {
            assert_eq!(
                unattached
                    .authorize(operation, no_authority, NOW)
                    .unwrap_err(),
                AuthorityError::NotAttached
            );
            assert!(active.authorize(operation, active_owner, NOW).is_ok());
            assert_eq!(
                grace.authorize(operation, no_authority, NOW).unwrap_err(),
                AuthorityError::NotAttached
            );
            assert!(held.authorize(operation, held_owner, NOW).is_ok());
        }

        assert!(
            unattached
                .authorize(AttachmentOperation::LeaseClaimTakeover, no_authority, NOW)
                .is_ok()
        );
        assert_eq!(
            active
                .authorize(AttachmentOperation::LeaseClaimTakeover, active_owner, NOW,)
                .unwrap_err(),
            AuthorityError::AlreadyAttached
        );
        assert_eq!(
            grace
                .authorize(AttachmentOperation::LeaseClaimTakeover, no_authority, NOW)
                .unwrap_err(),
            AuthorityError::NotAttached
        );
        assert!(
            held.authorize(
                AttachmentOperation::LeaseClaimTakeover,
                creds(Some("bob"), None, None),
                NOW,
            )
            .is_ok()
        );

        assert_eq!(
            unattached
                .authorize(AttachmentOperation::LeaseTouchRelease, no_authority, NOW)
                .unwrap_err(),
            AuthorityError::NotAttached
        );
        assert_eq!(
            active
                .authorize(AttachmentOperation::LeaseTouchRelease, active_owner, NOW,)
                .unwrap_err(),
            AuthorityError::NotAttached
        );
        assert_eq!(
            grace
                .authorize(AttachmentOperation::LeaseTouchRelease, no_authority, NOW,)
                .unwrap_err(),
            AuthorityError::NotAttached
        );
        assert!(
            held.authorize(AttachmentOperation::LeaseTouchRelease, held_owner, NOW,)
                .is_ok()
        );
    }

    #[test]
    fn authority_transitions_preserve_fencing_and_token_invariants() {
        let mut authority = AttachmentAuthority::unattached();
        authority
            .grant_active("alice", ReclaimTokenHash::from_token("first"))
            .unwrap();
        authority.detach(5_000, NOW).unwrap();
        assert_eq!(authority.state, AttachmentAuthorityState::Held);
        assert_eq!(authority.generation, 1);
        assert_eq!(authority.deadline_ms, Some(NOW + 5_000));
        assert!(authority.reclaim_token_hash.is_none());

        authority.touch_hold("alice", 1, 10_000, NOW + 100).unwrap();
        assert_eq!(authority.deadline_ms, Some(NOW + 10_100));
        assert_eq!(authority.generation, 1);

        authority
            .attach_from_hold(
                "alice",
                1,
                ReclaimTokenHash::from_token("second"),
                NOW + 200,
            )
            .unwrap();
        assert_eq!(authority.state, AttachmentAuthorityState::Active);
        assert_eq!(authority.generation, 2);
        assert!(
            authority
                .reclaim_token_hash
                .as_ref()
                .is_some_and(|hash| hash.matches("second"))
        );

        authority.enter_reclaim_grace(NOW + 20_000).unwrap();
        assert_eq!(authority.state, AttachmentAuthorityState::ReclaimGrace);
        authority
            .attach_from_reclaim("second", ReclaimTokenHash::from_token("third"), NOW + 300)
            .unwrap();
        assert_eq!(authority.state, AttachmentAuthorityState::Active);
        assert!(
            authority
                .reclaim_token_hash
                .as_ref()
                .is_some_and(|hash| hash.matches("third"))
        );
        assert!(
            !authority
                .reclaim_token_hash
                .as_ref()
                .is_some_and(|hash| hash.matches("second"))
        );

        authority.detach(0, NOW + 400).unwrap();
        assert_eq!(authority.state, AttachmentAuthorityState::Unattached);
        assert!(authority.owner.is_none());
        assert!(authority.reclaim_token_hash.is_none());
        assert_eq!(authority.generation, 2);
    }

    #[test]
    fn expired_grace_and_holds_are_effectively_unattached() {
        let grace = grace();
        assert_eq!(
            grace.state_at(NOW + 1_000),
            AttachmentAuthorityState::Unattached
        );
        assert!(
            grace
                .authorize(
                    AttachmentOperation::Attach,
                    creds(None, Some("active-token"), None),
                    NOW + 1_000,
                )
                .is_ok()
        );

        let held = held();
        assert_eq!(
            held.state_at(NOW + 1_000),
            AttachmentAuthorityState::Unattached
        );
        assert!(
            held.authorize(
                AttachmentOperation::Attach,
                creds(Some("alice"), None, Some(3)),
                NOW + 1_000,
            )
            .is_ok()
        );
    }
}
