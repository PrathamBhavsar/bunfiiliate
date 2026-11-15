-- A purchased variant must identify exactly one plan.
CREATE UNIQUE INDEX billing_plans_lemonsqueezy_variant_uniq
    ON billing_plans (lemonsqueezy_variant_id) WHERE lemonsqueezy_variant_id IS NOT NULL;
