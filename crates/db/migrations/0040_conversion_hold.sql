-- Why a pending conversion is held back from auto-approve (its attributing click was
-- flagged as fraud). Cleared by an admin release. `clicks.fraud_reason` already exists.
ALTER TABLE conversions ADD COLUMN hold_reason text;
