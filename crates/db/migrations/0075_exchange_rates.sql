-- B06: cached exchange rates. Only USD-based rows are written; cross rates are triangulated
-- through USD. Old rows stay for history and as the offline fallback.
CREATE TABLE exchange_rates (
    base       char(3) NOT NULL,
    quote      char(3) NOT NULL,
    rate       numeric(18,8) NOT NULL CHECK (rate > 0),
    as_of      date NOT NULL,
    fetched_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (base, quote, as_of),
    CHECK (base <> quote)
);
