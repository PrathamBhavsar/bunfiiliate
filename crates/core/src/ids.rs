use uuid::Uuid;

macro_rules! id {
    ($($name:ident),*) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);
    )*};
}

id!(ProgramId, AffiliateId, ConversionId, PayoutId, TxnId);
