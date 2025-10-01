use std::sync::Arc;

use crate::domain::ticket::{
    Ticket, TicketError, TicketFilter, TicketId, TicketPriority, TicketRepository, TicketResult,
    TicketStatus,
};
use crate::domain::user::UserId;

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
        self.repo.insert(ticket)
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
        self.repo.update(ticket)
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
        self.repo.update(ticket)
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
        self.repo.update(ticket)
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
        self.repo.update(ticket)
    }

    pub fn list(&self, filter: TicketFilter) -> TicketResult<Vec<Ticket>> {
        self.repo.list(&filter)
    }

    pub fn find_by_id(&self, ticket_id: &TicketId) -> TicketResult<Option<Ticket>> {
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
