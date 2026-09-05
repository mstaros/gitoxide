use std::borrow::Cow;

use super::Name;
use crate::bstr::{BStr, BString, ByteSlice, ByteVec};

/// The error returned by [validated()].
#[derive(Debug, thiserror::Error)]
#[error("remote names must be valid within refspecs for fetching: {name:?}")]
#[expect(missing_docs)]
pub struct Error {
    pub source: gix_refspec::parse::Error,
    pub name: BString,
}

/// Return `name` if it is valid as symbolic remote name.
///
/// This means it has to be valid within a the ref path of a tracking branch.
pub fn validated(name: impl Into<BString>) -> Result<BString, Error> {
    let name = name.into();
    let mut spec = BString::from("refs/heads/test:refs/remotes/");
    spec.push_str(name.as_bstr());
    spec.push_str("/test");
    match gix_refspec::parse(spec.as_bstr(), gix_refspec::parse::Operation::Fetch) {
        Ok(_) => Ok(name),
        Err(err) => Err(Error { source: err, name }),
    }
}

impl Name<'_> {
    /// Obtain the name as string representation.
    pub fn as_bstr(&self) -> &BStr {
        match self {
            Name::Symbol(v) | Name::Url(v) => v.as_ref(),
        }
    }

    /// Return this instance as a symbolic UTF-8 name, if it is one.
    /// For a non-UTF-8 symbolic name this returns `None`; use [Self::as_bstr()] for exact bytes.
    pub fn as_symbol(&self) -> Option<&str> {
        match self {
            Name::Symbol(n) => n.to_str().ok(),
            Name::Url(_) => None,
        }
    }

    /// Return this instance as url, if it is one.
    pub fn as_url(&self) -> Option<&BStr> {
        match self {
            Name::Url(n) => n.as_ref().into(),
            Name::Symbol(_) => None,
        }
    }

    /// Return a fully-owned copy of this instance.
    pub fn to_owned(&self) -> Name<'static> {
        match self {
            Name::Symbol(s) => Name::Symbol(s.clone().into_owned().into()),
            Name::Url(s) => Name::Url(s.clone().into_owned().into()),
        }
    }
}

fn classify(name: Cow<'_, BStr>) -> Name<'_> {
    if name.contains(&b'/') || name.as_ref() == "." {
        Name::Url(name)
    } else {
        Name::Symbol(name)
    }
}

impl<'a> TryFrom<Cow<'a, BStr>> for Name<'a> {
    type Error = Cow<'a, BStr>;

    fn try_from(name: Cow<'a, BStr>) -> Result<Self, Self::Error> {
        Ok(classify(name))
    }
}

impl From<BString> for Name<'static> {
    fn from(name: BString) -> Self {
        classify(Cow::Owned(name))
    }
}

impl AsRef<BStr> for Name<'_> {
    fn as_ref(&self) -> &BStr {
        self.as_bstr()
    }
}

mod impls {
    use crate::{
        bstr::{BStr, BString, ByteSlice},
        remote::Name,
    };

    macro_rules! impl_partial_eq {
        ($left_type:ty, $right_type:ty, $left:ident => $left_bytes:expr, $right:ident => $right_bytes:expr) => {
            impl PartialEq<$right_type> for $left_type {
                fn eq(&self, other: &$right_type) -> bool {
                    let $left = self;
                    let $right = other;
                    $left_bytes == $right_bytes
                }
            }
        };
    }

    macro_rules! impl_partial_eq_pair {
        ($left_type:ty, $right_type:ty, $left:ident => $left_bytes:expr, $right:ident => $right_bytes:expr) => {
            impl_partial_eq!($left_type, $right_type, $left => $left_bytes, $right => $right_bytes);
            impl_partial_eq!($right_type, $left_type, $right => $right_bytes, $left => $left_bytes);
        };
    }

    macro_rules! impl_partial_eq_bytes {
        ($type:ty, $value:ident => $bytes:expr) => {
            impl_partial_eq_pair!($type, str, $value => $bytes.as_bytes(), other => other.as_bytes());
            impl_partial_eq_pair!($type, &str, $value => $bytes.as_bytes(), other => other.as_bytes());
            impl_partial_eq_pair!($type, String, $value => $bytes.as_bytes(), other => other.as_bytes());
            impl_partial_eq_pair!($type, BStr, $value => $bytes.as_bytes(), other => other.as_bytes());
            impl_partial_eq_pair!($type, &BStr, $value => $bytes.as_bytes(), other => other.as_bytes());
            impl_partial_eq_pair!($type, BString, $value => $bytes.as_bytes(), other => other.as_bytes());
        };
    }

    impl_partial_eq_bytes!(Name<'_>, value => value.as_bstr());
    impl_partial_eq_pair!(
        &Name<'_>,
        String,
        name => name.as_bstr().as_bytes(),
        text => text.as_bytes()
    );
    impl_partial_eq_pair!(
        &Name<'_>,
        BString,
        name => name.as_bstr().as_bytes(),
        text => text.as_bytes()
    );
}
