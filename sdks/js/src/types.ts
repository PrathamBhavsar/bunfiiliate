/** Mirrors schemas/tracker/v2/event.schema.json. */
export type EventType = "sale" | "signup" | "lead" | "refund" | "chargeback" | `custom:${string}`;

export interface BfEvent {
  schema: "bf.event.v2";
  type: EventType;
  external_id: string;
  occurred_at?: string;
  click_id?: string;
  code?: string;
  customer?: { external_id?: string; email_sha256?: string };
  /** Integer cents. Required with `currency` for sale/refund/chargeback. */
  amount_cents?: number;
  currency?: string;
  ref_external_id?: string;
  meta?: Record<string, string | number | boolean>;
}

export interface SendResult {
  status: "accepted" | "duplicate";
  id: string;
}
