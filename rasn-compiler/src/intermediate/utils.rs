macro_rules! get_declaration {
    ($tlds:ident, $key:expr, $tld_ty:ident, $asn1_ty:path) => {{
        if let Some(tld) = $tlds.get($key) {
            match tld {
                ToplevelDefinition::$tld_ty(inner) => match inner.pdu() {
                    $asn1_ty(asn) => Some(asn),
                    _ => None,
                },
                _ => None,
            }
        } else {
            None
        }
    }};
}

use std::error::Error;

use crate::intermediate::{ModuleHeader, ToplevelDefinition};
use crate::lexer;
pub(crate) use get_declaration;

pub fn asn_spec(
    input: &str,
) -> Result<Vec<(ModuleHeader, Vec<ToplevelDefinition>)>, Box<dyn Error>> {
    lexer::asn_spec(input.into())
        .map_err(|lexer_error| format!("can not parse asn1 definition: {lexer_error:?}").into())
}
