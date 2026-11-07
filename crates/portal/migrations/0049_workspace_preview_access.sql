-- Who started a workspace's preview and the salt its slugs are minted with (CC-91, CC-92, T-5760).
-- Both are written when the preview starts and replaced on the next start; the gateway reads
-- them through the internal preview list, never through the workspace API.
ALTER TABLE workspaces ADD COLUMN IF NOT EXISTS preview_salt text;
ALTER TABLE workspaces ADD COLUMN IF NOT EXISTS preview_author text;
