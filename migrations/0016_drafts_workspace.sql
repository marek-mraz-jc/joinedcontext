-- A draft belongs to one workspace or to none (CC-76): the empty string is "none", so the
-- key stays a plain primary key and every existing draft is a draft of the main project.
ALTER TABLE drafts ADD COLUMN IF NOT EXISTS workspace text NOT NULL DEFAULT '';
ALTER TABLE drafts DROP CONSTRAINT IF EXISTS drafts_pkey;
ALTER TABLE drafts ADD PRIMARY KEY (project, kind, name, workspace);
