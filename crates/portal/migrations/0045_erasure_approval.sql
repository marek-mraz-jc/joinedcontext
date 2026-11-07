-- Four eyes on an erasure (API/01 §46, DM-142, T-5356): one owner proposes it, another person
-- with approve on the ContextSpace approves or declines it. A proposal erases nothing until it is
-- approved; one undecided for 7 days reads expired and is approved by nobody. The erasures logged
-- before this ran on one owner's call, so they count as approved.
ALTER TABLE erasures ADD COLUMN IF NOT EXISTS approval text NOT NULL DEFAULT 'approved'
  CHECK (approval IN ('proposed', 'approved', 'declined'));
ALTER TABLE erasures ALTER COLUMN approval SET DEFAULT 'proposed';
ALTER TABLE erasures ADD COLUMN IF NOT EXISTS decided_by text;
ALTER TABLE erasures ADD COLUMN IF NOT EXISTS decided_at timestamptz;
ALTER TABLE erasures ADD COLUMN IF NOT EXISTS decline_reason text
  CHECK (decline_reason IS NULL OR char_length(decline_reason) BETWEEN 1 AND 500);
