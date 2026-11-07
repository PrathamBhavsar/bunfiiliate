-- B02: a refund, dispute or chargeback finds its sale by provider ids (charge, invoice, payment
-- intent, Paddle transaction) inside the stored sale events. One index per id path keeps that
-- lookup off a scan of every event the program has received.
CREATE INDEX inbound_events_ref_id_idx
    ON inbound_events (program_id, source, (payload #>> '{data,object,id}'));
CREATE INDEX inbound_events_ref_charge_idx
    ON inbound_events (program_id, source, (payload #>> '{data,object,charge}'));
CREATE INDEX inbound_events_ref_invoice_idx
    ON inbound_events (program_id, source, (payload #>> '{data,object,invoice}'));
CREATE INDEX inbound_events_ref_intent_idx
    ON inbound_events (program_id, source, (payload #>> '{data,object,payment_intent}'));
CREATE INDEX inbound_events_ref_paddle_idx
    ON inbound_events (program_id, source, (payload #>> '{data,id}'));
