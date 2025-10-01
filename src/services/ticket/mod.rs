use std::sync::Arc;

use crate::domain::ticket::{
    Ticket, TicketError, TicketFilter, TicketId, TicketPriority, TicketRepository, TicketResult,
    TicketStatus,
};
use crate::domain::user::UserId;
use crate::infra::logging;
use tracing::info;

#[derive(Clone)]
pub struct TicketService {
    repo: Arc<dyn TicketRepository>,
}

impl TicketService {
    pub fn new(repo: Arc<dyn TicketRepository>) -> Self {
        Self { repo }
    }

    pub fn create(&self, cmd: CreateTicketCommand) -> TicketResult<Ticket> {
        let ticket = Ticket::new(
            TicketId::new(),
            cmd.title,
            cmd.description,
            cmd.priority,
            cmd.reporter_id,
            cmd.assignee_id,
            cmd.tags,
        )?;
        let inserted = self.repo.insert(ticket)?;
        info!(ticket_id = %inserted.id, status = %inserted.status, priority = %inserted.priority, "ticket created");
        logging::append_db_log(&format!(
            "INSERT tickets id={} status={} priority={} reporter={} assignee={:?}",
            inserted.id,
            inserted.status,
            inserted.priority,
            inserted.reporter_id,
            inserted.assignee_id
        ));
        Ok(inserted)
    }

    pub fn assign(&self, ticket_id: &TicketId, assignee: Option<UserId>) -> TicketResult<Ticket> {
        let mut ticket = self
            .repo
            .find_by_id(ticket_id)?
            .ok_or(TicketError::NotFound)?;
        if assignee == ticket.assignee_id {
            return Ok(ticket);
        }
        ticket.set_assignee(assignee);
        let updated = self.repo.update(ticket)?;
        info!(ticket_id = %updated.id, assignee = ?updated.assignee_id.map(|id| id.to_string()), "ticket assignee updated");
        logging::append_db_log(&format!(
            "UPDATE tickets id={} set assignee={:?}",
            updated.id, updated.assignee_id
        ));
        Ok(updated)
    }

    pub fn transition_status(
        &self,
        ticket_id: &TicketId,
        status: TicketStatus,
    ) -> TicketResult<Ticket> {
        let mut ticket = self
            .repo
            .find_by_id(ticket_id)?
            .ok_or(TicketError::NotFound)?;
        ticket.set_status(status);
        let updated = self.repo.update(ticket)?;
        info!(ticket_id = %updated.id, status = %updated.status, "ticket status updated");
        logging::append_db_log(&format!(
            "UPDATE tickets id={} set status={}",
            updated.id, updated.status
        ));
        Ok(updated)
    }

    pub fn update_priority(
        &self,
        ticket_id: &TicketId,
        priority: TicketPriority,
    ) -> TicketResult<Ticket> {
        let mut ticket = self
            .repo
            .find_by_id(ticket_id)?
            .ok_or(TicketError::NotFound)?;
        ticket.set_priority(priority);
        let updated = self.repo.update(ticket)?;
        info!(ticket_id = %updated.id, priority = %updated.priority, "ticket priority updated");
        logging::append_db_log(&format!(
            "UPDATE tickets id={} set priority={}",
            updated.id, updated.priority
        ));
        Ok(updated)
    }

    pub fn update_details(
        &self,
        ticket_id: &TicketId,
        title: Option<String>,
        description: Option<String>,
        tags: Option<Vec<String>>,
    ) -> TicketResult<Ticket> {
        let mut ticket = self
            .repo
            .find_by_id(ticket_id)?
            .ok_or(TicketError::NotFound)?;
        if let Some(title) = title {
            ticket.update_title(title)?;
        }
        if let Some(description) = description {
            ticket.update_description(description)?;
        }
        if let Some(tags) = tags {
            ticket.set_tags(tags);
        }
        let updated = self.repo.update(ticket)?;
        info!(ticket_id = %updated.id, "ticket details updated");
        logging::append_db_log(&format!(
            "UPDATE tickets id={} set title=\"{}\" description_len={} tags={:?}",
            updated.id,
            updated.title,
            updated.description.len(),
            updated.tags
        ));
        Ok(updated)
    }

    pub fn list(&self, filter: TicketFilter) -> TicketResult<Vec<Ticket>> {
        logging::append_db_log(&format!("SELECT tickets filter={:?}", filter));
        let result = self.repo.list(&filter);
        if let Ok(ref tickets) = result {
            info!(count = tickets.len(), "ticket list returned");
        }
        result
    }

    pub fn find_by_id(&self, ticket_id: &TicketId) -> TicketResult<Option<Ticket>> {
        logging::append_db_log(&format!("SELECT ticket by id={}", ticket_id));
        self.repo.find_by_id(ticket_id)
    }
}

pub struct CreateTicketCommand {
    pub title: String,
    pub description: String,
    pub priority: TicketPriority,
    pub reporter_id: UserId,
    pub assignee_id: Option<UserId>,
    pub tags: Vec<String>,
}

impl CreateTicketCommand {
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        priority: TicketPriority,
        reporter_id: UserId,
    ) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            priority,
            reporter_id,
            assignee_id: None,
            tags: Vec::new(),
        }
    }

    pub fn with_assignee(mut self, assignee: Option<UserId>) -> Self {
        self.assignee_id = assignee;
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}
