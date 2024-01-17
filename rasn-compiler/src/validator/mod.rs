//! The `validator` module ensures that the
//! parsed data elements of the ASN1 specification
//! are valid declarations that can be further
//! processed. Among other things, the `validator`
//! assures that all dependencies of the individual
//! data elements resolve, and checks for conflicting
//! constraints and value definitions.
pub(crate) mod error;
mod linking;

use std::{collections::BTreeMap, error::Error, ops::Not};

use crate::intermediate::{constraints::*, types::*, *, information_object::{ClassLink, ToplevelInformationDeclaration}};

use self::error::{ValidatorError, ValidatorErrorType};

pub struct Validator {
    tlds: BTreeMap<String, ToplevelDeclaration>,
}

impl Validator {
    pub fn new(tlds: Vec<ToplevelDeclaration>) -> Validator {
        Self {
            tlds: tlds
                .into_iter()
                .map(|tld| (tld.name().to_owned(), tld))
                .collect(),
        }
    }

    fn link(mut self) -> Result<(Self, Vec<Box<dyn Error>>), ValidatorError> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];
        // Linking of ASN1 values depends on linked ASN1 types, so we order the key colelction accordingly (note that we pop keys)
        let mut keys = self.tlds.iter()
            .filter_map(|(k, v)| matches![v, ToplevelDeclaration::Value(_)].then_some(k.clone()))
            .chain(
                self.tlds.iter()
                .filter_map(|(k, v)| matches![v, ToplevelDeclaration::Value(_)].not().then_some(k.clone()))
            ).collect::<Vec<String>>();
        while let Some(key) = keys.pop() {
            if self.references_class_by_name(&key) {
                match self.tlds.remove(&key) {
                    Some(ToplevelDeclaration::Type(mut tld)) => {
                        tld.r#type = tld.r#type.resolve_class_reference(&self.tlds);
                        self.tlds
                        .insert(tld.name.clone(), ToplevelDeclaration::Type(tld));
                        },
                    Some(ToplevelDeclaration::Information(mut tld)) => {
                        tld = tld.resolve_class_reference(&self.tlds);
                        self.tlds
                        .insert(tld.name.clone(), ToplevelDeclaration::Information(tld));
                    },
                    _ => ()
                }
            }
            if self.has_components_of_notation(&key) {
                if let Some(ToplevelDeclaration::Type(mut tld)) = self.tlds.remove(&key) {
                    tld.r#type.link_components_of_notation(&self.tlds);
                    self.tlds
                        .insert(tld.name.clone(), ToplevelDeclaration::Type(tld));
                }
            } 
            if self.has_choice_selection_type(&key) {
                if let Some(ToplevelDeclaration::Type(mut tld)) = self.tlds.remove(&key) {
                    tld.r#type.link_choice_selection_type(&self.tlds)?;
                    self.tlds
                        .insert(tld.name.clone(), ToplevelDeclaration::Type(tld));
                }
            }
            if self.references_object_set_by_name(&key) {
                if let Some(ToplevelDeclaration::Information(mut tld)) = self.tlds.remove(&key) {
                    tld.value.link_object_set_reference(&self.tlds);
                    self.tlds
                        .insert(tld.name.clone(), ToplevelDeclaration::Information(tld));
                }
            }
            if self.has_constraint_reference(&key) {
                let mut tld = self.tlds.remove(&key).ok_or(ValidatorError {
                    data_element: Some(key.clone()),
                    details: "Could not find toplevel declaration to remove!".into(),
                    kind: ValidatorErrorType::MissingDependency,
                })?;
                if !tld.link_constraint_reference(&self.tlds)? {
                    warnings.push(
                        Box::new(
                            ValidatorError { 
                                data_element: Some(tld.name().to_string()), 
                                details: format!(
                                    "Failed to link cross-reference to elsewhere defined value in constraint of {}", 
                                    tld.name()),
                                kind: ValidatorErrorType::MissingDependency
                            }
                        )
                    )
                }
                self.tlds.insert(tld.name().clone(), tld);
            }
            if let Some(mut tld) = self.tlds.remove(&key) {
                tld.collect_supertypes(&self.tlds)?;
                self.tlds.insert(key, tld);
            }
        }

        Ok((self, warnings))
    }

    fn has_constraint_reference(&mut self, key: &String) -> bool {
        self.tlds
            .get(key)
            .map(ToplevelDeclaration::has_constraint_reference)
            .unwrap_or(false)
    }

    fn references_class_by_name(&mut self, key: &String) -> bool {
        self.tlds
            .get(key)
            .map(|t| match t {
                ToplevelDeclaration::Type(t) => t.r#type.references_class_by_name(),
                ToplevelDeclaration::Information(i) => i.class.as_ref().map_or(false, |c| match c {
                    ClassLink::ByReference(_) => false,
                    ClassLink::ByName(_) => true
                }),
                _ => false,
            })
            .unwrap_or(false)
    }

    fn references_object_set_by_name(&mut self, key: &String) -> bool {
        self.tlds
            .get(key)
            .map(|t|
                match t {
                ToplevelDeclaration::Information(ToplevelInformationDeclaration { value, .. }) => value.references_object_set_by_name(),
                _ => false,
            })
            .unwrap_or(false)
    }

    fn has_choice_selection_type(&mut self, key: &String) -> bool {
        self.tlds
            .get(key)
            .map(|t| match t {
                ToplevelDeclaration::Type(t) => t.r#type.has_choice_selection_type(),
                _ => false,
            })
            .unwrap_or(false)
    }

    fn has_components_of_notation(&mut self, key: &String) -> bool {
        self.tlds
            .get(key)
            .map(|t| match t {
                ToplevelDeclaration::Type(t) => t.r#type.contains_components_of_notation(),
                _ => false,
            })
            .unwrap_or(false)
    }

    pub fn validate(
        mut self,
    ) -> Result<(Vec<ToplevelDeclaration>, Vec<Box<dyn Error>>), Box<dyn Error>> {
        let warnings: Vec<Box<dyn Error>>;
        (self, warnings) = self.link()?;
        Ok(self.tlds.into_iter().fold(
            (Vec::<ToplevelDeclaration>::new(), warnings),
            |(mut tlds, mut errors), (_, tld)| {
                match tld.validate() {
                    Ok(_) => tlds.push(tld),
                    Err(e) => errors.push(Box::new(e)),
                }
                (tlds, errors)
            },
        ))
    }
}

pub trait Validate {
    fn validate(&self) -> Result<(), ValidatorError>;
}

impl Validate for ToplevelDeclaration {
    fn validate(&self) -> Result<(), ValidatorError> {
        match self {
            ToplevelDeclaration::Type(t) => {
                if let Err(mut e) = t.r#type.validate() {
                    e.specify_data_element(t.name.clone());
                    return Err(e);
                }
                Ok(())
            }
            ToplevelDeclaration::Value(_v) => Ok(()),
            ToplevelDeclaration::Information(_i) => Ok(()),
        }
    }
}

impl Validate for ASN1Type {
    fn validate(&self) -> Result<(), ValidatorError> {
        match self {
            ASN1Type::Integer(ref i) => i.validate(),
            ASN1Type::BitString(ref b) => b.validate(),
            ASN1Type::CharacterString(ref o) => o.validate(),
            _ => Ok(()),
        }
    }
}

impl Validate for Integer {
    fn validate(&self) -> Result<(), ValidatorError> {
        for c in &self.constraints {
            c.validate()?;
        }
        Ok(())
    }
}

impl Validate for BitString {
    fn validate(&self) -> Result<(), ValidatorError> {
        for c in &self.constraints {
            c.validate()?;
        }
        Ok(())
    }
}

impl Validate for CharacterString {
    fn validate(&self) -> Result<(), ValidatorError> {
        for c in &self.constraints {
            c.validate()?;
        }
        Ok(())
    }
}

impl Validate for Constraint {
    fn validate(&self) -> Result<(), ValidatorError> {
        if let Constraint::SubtypeConstraint(c) = self {
            if let ElementOrSetOperation::Element(SubtypeElement::ValueRange {
                min,
                max,
                extensible: _,
            }) = &c.set
            {
                if let Some((ASN1Value::Integer(min), ASN1Value::Integer(max))) =
                    min.as_ref().zip(max.as_ref())
                {
                    if min > max {
                        return Err(ValidatorError::new(
                            None,
                            "Mininum value exceeds maximum value!",
                            ValidatorErrorType::InvalidConstraintsError,
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
