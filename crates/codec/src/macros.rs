//! `cbor_struct!`: the keyasint struct declarations of wire, ticket, view and admin.

/// Declares a keyasint CBOR struct. Fields are listed in ascending key order as
/// `key => field: Type = "go type"`, with `[omitempty]` where the Go tag has it:
///
/// ```
/// dstore_codec::cbor_struct! {
///     #[derive(Clone, Debug, Default, PartialEq)]
///     pub struct RefInfo = "wire.RefInfo" {
///         0 => name: String = "string",
///         1 => key: Option<Vec<u8>> = "[]uint8",
///         3 => created_at: i64 = "int64",
///         4 => user: String = "string" [omitempty],
///     }
/// }
/// ```
///
/// Generates the struct (all fields `pub`, with the given attributes, which must include
/// `derive(Default)`), `Encode` (count present fields, shortest map head, ascending keys) and
/// `Struct::decode_struct` (definite or indefinite map, numeric key dispatch, first duplicate wins,
/// errors rewritten to "<GO_NAME>.<key>").
///
/// The encoder writes the fields in declaration order, so the keys must be strictly ascending. A
/// compile-time assertion refuses anything else:
///
/// ```compile_fail
/// dstore_codec::cbor_struct! {
///     #[derive(Default)]
///     pub struct Unordered = "x.Unordered" {
///         1 => a: u64 = "uint64",
///         0 => b: u64 = "uint64",
///     }
/// }
/// ```
#[macro_export]
macro_rules! cbor_struct {
    (@count $n:ident, [], $v:expr) => {
        $n += 1;
    };
    (@count $n:ident, [omitempty], $v:expr) => {
        if !$crate::Field::is_empty_field($v) {
            $n += 1;
        }
    };
    (@field $e:ident, $key:literal, [], $v:expr) => {
        $e.uint($key);
        $crate::Field::encode_field($v, $e);
    };
    (@field $e:ident, $key:literal, [omitempty], $v:expr) => {
        if !$crate::Field::is_empty_field($v) {
            $e.uint($key);
            $crate::Field::encode_field($v, $e);
        }
    };
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident = $go_name:literal {
            $( $key:literal => $field:ident : $ty:ty = $go_type:literal $([$omit:ident])? ),+ $(,)?
        }
    ) => {
        $(#[$attr])*
        $vis struct $name {
            $( pub $field: $ty, )+
        }

        // Canonical encoding writes the fields in declaration order: the keys must ascend.
        const _: () = {
            let keys: &[u64] = &[$($key),+];
            let mut i = 1;
            while i < keys.len() {
                assert!(
                    keys[i - 1] < keys[i],
                    "cbor_struct!: field keys must be strictly ascending"
                );
                i += 1;
            }
        };

        impl $crate::Encode for $name {
            fn encode(&self, e: &mut $crate::Enc) {
                let mut present: u64 = 0;
                $( $crate::cbor_struct!(@count present, [$($omit)?], &self.$field); )+
                e.head(5, present);
                $( $crate::cbor_struct!(@field e, $key, [$($omit)?], &self.$field); )+
            }
        }

        impl $crate::Struct for $name {
            const GO_NAME: &'static str = $go_name;

            fn decode_struct(
                d: &mut $crate::Dec<'_>,
            ) -> ::core::result::Result<Self, $crate::DecodeError> {
                let mut v = <Self as ::core::default::Default>::default();
                d.read_map_struct($go_name, |d, key| match key {
                    $(
                        $key => {
                            v.$field = <$ty as $crate::Field>::decode_field(d, $go_type)?;
                            ::core::result::Result::Ok(true)
                        }
                    )+
                    _ => ::core::result::Result::Ok(false),
                })?;
                ::core::result::Result::Ok(v)
            }
        }
    };
}
