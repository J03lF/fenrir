-- Athenos work management baseline schema
-- Adds projects, SLA policies, and tickets tables.

CREATE TABLE IF NOT EXISTS athenos_projects (
  id UUID PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  description TEXT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS athenos_sla_policies (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  response_target_minutes INTEGER NOT NULL,
  resolution_target_minutes INTEGER NOT NULL,
  applies_to_priorities JSONB NOT NULL DEFAULT '[]'::jsonb,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS athenos_tickets (
  id UUID PRIMARY KEY,
  project_id UUID NOT NULL REFERENCES athenos_projects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  priority TEXT NOT NULL DEFAULT 'medium',
  assignee_id UUID NULL,
  reporter_id UUID NOT NULL,
  sla_policy_id UUID NULL REFERENCES athenos_sla_policies(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_athenos_tickets_project_id ON athenos_tickets(project_id);
CREATE INDEX IF NOT EXISTS idx_athenos_tickets_status ON athenos_tickets(status);
