use std::collections::BTreeMap;

use crate::intermediate::{
    error::{GrammarError, GrammarErrorType},
    information_object::*,
    *,
};

pub(crate) fn find_tld_or_enum_value_by_name(
    type_name: &String,
    name: &String,
    tlds: &BTreeMap<String, ToplevelDeclaration>,
) -> Option<ASN1Value> {
    if let Some(ToplevelDeclaration::Value(v)) = tlds.get(name) {
        return Some(v.value.clone());
    } else {
        for (_, tld) in tlds.iter() {
            if let Some(value) = tld.get_distinguished_or_enum_value(Some(type_name), name) {
                return Some(value);
            }
        }
        // Make second attempt without requiring a matching type name
        // This is the current best shot at linking inner subtypes
        for (_, tld) in tlds.iter() {
            if let Some(value) = tld.get_distinguished_or_enum_value(None, name) {
                return Some(value);
            }
        }
    }
    None
}

pub(crate) fn bit_string_to_octet_string(bits: &Vec<bool>) -> Result<Vec<u8>, GrammarError> {
    let mut octets = vec![];
    for byte in bits.chunks(8) {
        if byte.len() != 8 {
            return Err(GrammarError {
                details: "Binary octet string value needs to be a multiple of 8 bits!".into(),
                kind: GrammarErrorType::LinkerError,
            });
        }
        octets.push(byte.iter().enumerate().fold(0u8, |acc, (i, bit)| {
            acc + if *bit { 2u8.pow(7 - i as u32) } else { 0 }
        }));
    }
    Ok(octets)
}

pub(crate) fn walk_object_field_ref_path<'a>(
    fields: &'a Vec<InformationObjectClassField>,
    path: &'a Vec<ObjectFieldIdentifier>,
    mut index: usize,
) -> Option<&'a InformationObjectClassField> {
    fields
        .iter()
        .find_map(|f| {
            path.get(index)
                .map(|id| {
                    (&f.identifier == id).then(|| {
                        if path.len() == (index + 1) {
                            Some(f)
                        } else {
                            index += 1;
                            walk_object_field_ref_path(fields, path, index)
                        }
                    })
                })
                .flatten()
        })
        .flatten()
}
