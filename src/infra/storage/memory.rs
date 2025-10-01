use std::collections::HashMap;
use std::sync::RwLock;

use crate::domain::ticket::{
    Ticket, TicketError, TicketFilter, TicketId, TicketRepository, TicketResult,
};
use crate::domain::user::{
    EmailAddress, User, UserError, UserFilter, UserId, UserRepository, UserResult,
};

#[derive(Default)]
struct UserStore {
    by_id: HashMap<UserId, User>,
    by_username: HashMap<String, UserId>,
    by_email: HashMap<String, UserId>,
}

pub struct InMemoryUserRepository {
    store: RwLock<UserStore>,
}

impl InMemoryUserRepository {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(UserStore::default()),
        }
    }

    fn with_store<T>(&self, f: impl FnOnce(&mut UserStore) -> UserResult<T>) -> UserResult<T> {
        let mut guard = self
            .store
            .write()
            .map_err(|_| UserError::storage("User-Store Lock wurde vergiftet"))?;
        f(&mut guard)
    }
}

impl Default for InMemoryUserRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl UserRepository for InMemoryUserRepository {
    fn insert(&self, user: User) -> UserResult<User> {
        self.with_store(|store| {
            if store.by_username.contains_key(&user.username) {
                return Err(UserError::DuplicateUsername(user.username.clone()));
            }
            if store.by_email.contains_key(user.email.as_str()) {
                return Err(UserError::DuplicateEmail(user.email.to_string()));
            }
            store.by_username.insert(user.username.clone(), user.id);
            store
                .by_email
                .insert(user.email.as_str().to_string(), user.id);
            store.by_id.insert(user.id, user.clone());
            Ok(user)
        })
    }

    fn update(&self, user: User) -> UserResult<User> {
        self.with_store(|store| {
            let entry = store.by_id.get_mut(&user.id).ok_or(UserError::NotFound)?;

            if entry.username != user.username {
                if store.by_username.contains_key(&user.username) {
                    return Err(UserError::DuplicateUsername(user.username.clone()));
                }
                store.by_username.remove(&entry.username);
                store.by_username.insert(user.username.clone(), user.id);
            }

            if entry.email != user.email {
                if store.by_email.contains_key(user.email.as_str()) {
                    return Err(UserError::DuplicateEmail(user.email.to_string()));
                }
                store.by_email.remove(entry.email.as_str());
                store
                    .by_email
                    .insert(user.email.as_str().to_string(), user.id);
            }

            *entry = user.clone();
            Ok(user)
        })
    }

    fn find_by_id(&self, id: &UserId) -> UserResult<Option<User>> {
        let guard = self
            .store
            .read()
            .map_err(|_| UserError::storage("User-Store Lock wurde vergiftet"))?;
        Ok(guard.by_id.get(id).cloned())
    }

    fn find_by_username(&self, username: &str) -> UserResult<Option<User>> {
        let guard = self
            .store
            .read()
            .map_err(|_| UserError::storage("User-Store Lock wurde vergiftet"))?;
        if let Some(id) = guard.by_username.get(username) {
            Ok(guard.by_id.get(id).cloned())
        } else {
            Ok(None)
        }
    }

    fn find_by_email(&self, email: &EmailAddress) -> UserResult<Option<User>> {
        let guard = self
            .store
            .read()
            .map_err(|_| UserError::storage("User-Store Lock wurde vergiftet"))?;
        if let Some(id) = guard.by_email.get(email.as_str()) {
            Ok(guard.by_id.get(id).cloned())
        } else {
            Ok(None)
        }
    }

    fn list(&self, filter: &UserFilter) -> UserResult<Vec<User>> {
        let guard = self
            .store
            .read()
            .map_err(|_| UserError::storage("User-Store Lock wurde vergiftet"))?;
        let mut users: Vec<User> = guard
            .by_id
            .values()
            .filter(|user| filter.include_locked || !user.is_locked)
            .filter(|user| match filter.role {
                Some(ref role) => user.roles.contains(role),
                None => true,
            })
            .filter(|user| match filter.search.as_ref() {
                Some(search) => {
                    let search = search.to_ascii_lowercase();
                    user.username.to_ascii_lowercase().contains(&search)
                        || user
                            .display_name
                            .as_ref()
                            .map(|name| name.to_ascii_lowercase().contains(&search))
                            .unwrap_or(false)
                        || user.email.as_str().contains(&search)
                }
                None => true,
            })
            .cloned()
            .collect();
        users.sort_by_key(|user| user.username.clone());
        Ok(users)
    }
}

#[derive(Default)]
struct TicketStore {
    by_id: HashMap<TicketId, Ticket>,
}

pub struct InMemoryTicketRepository {
    store: RwLock<TicketStore>,
}

impl InMemoryTicketRepository {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(TicketStore::default()),
        }
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&mut TicketStore) -> TicketResult<T>,
    ) -> TicketResult<T> {
        let mut guard = self
            .store
            .write()
            .map_err(|_| TicketError::storage("Ticket-Store Lock wurde vergiftet"))?;
        f(&mut guard)
    }
}

impl Default for InMemoryTicketRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketRepository for InMemoryTicketRepository {
    fn insert(&self, ticket: Ticket) -> TicketResult<Ticket> {
        self.with_store(|store| {
            if store.by_id.contains_key(&ticket.id) {
                return Err(TicketError::Conflict(format!(
                    "Ticket {} existiert bereits",
                    ticket.id
                )));
            }
            store.by_id.insert(ticket.id, ticket.clone());
            Ok(ticket)
        })
    }

    fn update(&self, ticket: Ticket) -> TicketResult<Ticket> {
        self.with_store(|store| {
            if !store.by_id.contains_key(&ticket.id) {
                return Err(TicketError::NotFound);
            }
            store.by_id.insert(ticket.id, ticket.clone());
            Ok(ticket)
        })
    }

    fn find_by_id(&self, id: &TicketId) -> TicketResult<Option<Ticket>> {
        let guard = self
            .store
            .read()
            .map_err(|_| TicketError::storage("Ticket-Store Lock wurde vergiftet"))?;
        Ok(guard.by_id.get(id).cloned())
    }

    fn list(&self, filter: &TicketFilter) -> TicketResult<Vec<Ticket>> {
        let guard = self
            .store
            .read()
            .map_err(|_| TicketError::storage("Ticket-Store Lock wurde vergiftet"))?;
        let mut tickets: Vec<Ticket> = guard
            .by_id
            .values()
            .filter(|ticket| match filter.status.as_ref() {
                Some(statuses) if !statuses.is_empty() => statuses.contains(&ticket.status),
                _ => true,
            })
            .filter(|ticket| match filter.reporter {
                Some(id) => ticket.reporter_id == id,
                None => true,
            })
            .filter(|ticket| match filter.assignee {
                Some(id) => ticket.assignee_id == Some(id),
                None => true,
            })
            .filter(|ticket| {
                if filter.tags.is_empty() {
                    return true;
                }
                let ticket_tags = ticket.tags.iter().collect::<Vec<_>>();
                filter
                    .tags
                    .iter()
                    .map(|tag| tag.to_ascii_lowercase())
                    .all(|tag| ticket_tags.iter().any(|t| t.as_str() == tag))
            })
            .filter(|ticket| match filter.search.as_ref() {
                Some(search) => {
                    let search = search.to_ascii_lowercase();
                    ticket.title.to_ascii_lowercase().contains(&search)
                        || ticket.description.to_ascii_lowercase().contains(&search)
                }
                None => true,
            })
            .cloned()
            .collect();
        tickets.sort_by_key(|ticket| ticket.created_at);
        tickets.reverse();
        Ok(tickets)
    }
}

fn _assert_send_sync_repo() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InMemoryUserRepository>();
    assert_send_sync::<InMemoryTicketRepository>();
}
