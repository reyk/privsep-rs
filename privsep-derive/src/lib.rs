//! Helper macros for the [`privsep`] create.
//!
//! [`privsep`]: http://docs.rs/privsep/

use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use std::collections::{HashMap, HashSet};
use syn::{
    parse::Parse, parse_macro_input, Attribute, Error, ItemEnum, Lit, LitStr, Meta, MetaList,
    MetaNameValue, NestedMeta, Path,
};

/// Derive privsep processes from an enum.
///
/// Attributes:
/// - `connect`: Connect child with the specified peer.
/// - `main_path`: Set the path of the parent or process `main` function.
/// - `username`: Set the default or the per-process privdrop user.
/// - `disable_privdrop`: disable privdrop for the program or process.
#[proc_macro_derive(Privsep, attributes(connect, main_path, username, disable_privdrop))]
pub fn derive_privsep(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(item as ItemEnum);

    derive_privsep_enum(input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn parse_attribute_value(attrs: &[Attribute], name: &str) -> Result<Option<LitStr>, Error> {
    if let Some(attr) = attrs.iter().find(|attr| attr.path.is_ident(name)) {
        match attr.parse_meta()? {
            Meta::NameValue(MetaNameValue {
                lit: Lit::Str(lit_str),
                ..
            }) => Ok(Some(lit_str)),
            meta => Err(Error::new_spanned(
                meta,
                &format!("invalid `{}` attribute", name),
            )),
        }
    } else {
        Ok(None)
    }
}

fn parse_attribute_ident(attrs: &[Attribute], name: &str) -> Result<Vec<Ident>, Error> {
    let mut result = vec![];

    // TODO: could we use `darling` here?
    if let Some(attr) = attrs.iter().find(|attr| attr.path.is_ident(name)) {
        match attr.parse_meta()? {
            Meta::List(MetaList { nested, .. }) => {
                for nested in nested.iter() {
                    if let NestedMeta::Meta(Meta::Path(path)) = nested {
                        if let Some(ident) = path.get_ident() {
                            result.push(ident.clone());
                        }
                    }
                }
            }
            ref meta => {
                return Err(Error::new_spanned(
                    meta,
                    &format!("invalid `{}` attribute", name),
                ))
            }
        }
    }

    Ok(result)
}

fn parse_attribute_type<T: Parse + ToTokens>(
    attrs: &[Attribute],
    name: &str,
    default: &str,
) -> Result<T, Error> {
    parse_attribute_value(attrs, name)?
        .unwrap_or_else(|| LitStr::new(default, Span::call_site()))
        .parse()
}

fn derive_privsep_enum(item: ItemEnum) -> Result<TokenStream, Error> {
    let ident = item.ident.clone();
    let attrs = &item.attrs;
    let mut as_ref_str = vec![];
    let mut child_main = vec![];
    let mut child_peers = vec![];
    let mut const_as_array = vec![];
    let mut const_id = vec![];
    let mut const_ids = vec![];
    let mut const_names = vec![];
    let mut child_names = vec![];
    let mut from_id = vec![];
    let mut children = vec![];
    let mut connect_map = HashMap::new();
    let not_connected = HashSet::new();
    let array_len = item.variants.len();

    // Get the global attributes.
    let disable_privdrop = attrs.iter().any(|a| a.path.is_ident("disable_privdrop"));
    let username = if let Some(username) = parse_attribute_value(attrs, "username")? {
        username
    } else if disable_privdrop {
        LitStr::new("", Span::call_site())
    } else {
        return Err(Error::new_spanned(
            item,
            "`Privsep` requires `username` attribute",
        ));
    };
    let doc = attrs
        .iter()
        .filter(|a| a.path.is_ident("doc"))
        .collect::<Vec<_>>();

    // Resolve bi-directional connections between processes.
    for variant in item.variants.iter() {
        let child_ident = variant.ident.clone();
        children.push(child_ident.clone());

        let connect = parse_attribute_ident(&variant.attrs, "connect")?
            .into_iter()
            .collect::<HashSet<_>>();
        connect_map.insert(child_ident, connect);
    }

    let temp_map = connect_map.clone();
    for (key, value) in temp_map.into_iter() {
        for entry in value.iter() {
            if !children.contains(entry) {
                return Err(Error::new_spanned(
                    item,
                    &format!("Connection to unknown process `{}`", entry),
                ));
            }
            if let Some(other) = connect_map.get_mut(entry) {
                other.insert(key.clone());
            }
        }
    }

    let mut main_path = quote! {
        unimplemented!()
    };
    let mut options = quote! {
        Options {
            config,
            ..Default::default()
        }
    };

    // Configure processes.
    for (id, variant) in item.variants.iter().enumerate() {
        let child_doc = variant
            .attrs
            .iter()
            .filter(|a| a.path.is_ident("doc"))
            .collect::<Vec<_>>();
        let child_ident = &variant.ident;
        let name_ident = child_ident.to_string();
        let name = name_ident.to_case(Case::Kebab);
        let name_snake = name_ident.to_case(Case::Snake);
        let name_upper = name_ident.to_case(Case::UpperSnake);
        let id_name = Ident::new(&(name_upper + "_ID"), Span::call_site());
        let child_main_path: Path =
            parse_attribute_type(&variant.attrs, "main_path", &(name_snake + "::main"))?;

        let child_username =
            parse_attribute_value(&variant.attrs, "username")?.unwrap_or_else(|| username.clone());
        let child_disable_privdrop =
            disable_privdrop || attrs.iter().any(|a| a.path.is_ident("disable_privdrop"));
        let child_options = quote! {
            privsep::process::Options {
                config: config.clone(),
                disable_privdrop: #child_disable_privdrop,
                username: #child_username.into(),
            }
        };
        child_names.push(name.clone());

        let connect = connect_map.get(child_ident).unwrap_or(&not_connected);

        let child_connect = children
            .iter()
            .enumerate()
            .map(|(id, child)| {
                let is_connected = id == 0 || connect.contains(child);
                quote! {
                    Process {
                        name: Self::as_static_str(&Self::#child),
                        connect: #is_connected
                    },
                }
            })
            .collect::<Vec<_>>();

        let is_child = id != 0;

        const_as_array.push(quote! {
            Process { name: #name, connect: #is_child },
        });

        const_id.push(quote! {
            #(#child_doc)*
            pub const #id_name: usize = #id;
        });

        const_ids.push(quote! {
            #id,
        });

        const_names.push(quote! {
            #name,
        });

        as_ref_str.push(quote! {
            Self::#child_ident => #name,
        });

        from_id.push(quote! {
            #id => Ok(Self::#child_ident),
        });

        child_peers.push(quote! {
            [#(#child_connect)*],
        });

        if is_child {
            let process = quote! {
                Child::<#array_len>::new([#(#child_connect)*], #name, &#child_options).await?
            };
            child_main.push(quote! {
                #name => {
                    let process = #process;
                    #child_main_path(process, config).await
                }
            });
        } else {
            options = child_options;
            main_path = quote! {
                #child_main_path
            };
        }
    }
    let child_main = child_main.into_iter().rev().collect::<Vec<_>>();

    if child_names.first().map(AsRef::as_ref) != Some("parent") {
        return Err(Error::new_spanned(
            item.variants,
            "Missing `Parent` variant",
        ));
    }

    Ok(quote! {
        #(#doc)*
        impl #ident {
            #(#const_id)*

            #[doc = "IDs of all child processes."]
            pub const PROCESS_IDS: [usize; #array_len] = [#(#const_ids)*];

            #[doc = "Names of all child processes."]
            pub const PROCESS_NAMES: [&'static str; #array_len] = [#(#const_names)*];

            #[doc = "Return processes as const list."]
            pub const fn as_array() -> [privsep::process::Process; #array_len] {
                use privsep::process::Process;
                [
                    #(#const_as_array)*
                ]
            }

            #[doc = "Start parent or child process."]
            pub async fn main(config: privsep::Config) -> Result<(), privsep::Error> {
                use privsep::process::{Child, Parent, Process};
                let name = std::env::args().next().unwrap_or_default();
                match name.as_ref() {
                    #(#child_main)*
                    _ => {
                        let process = Parent::new(Self::as_array(), &#options).await?;
                        #main_path(process.connect([#(#child_peers)*]).await?, config).await
                    }
                }
            }

            pub const fn as_static_str(&self) -> &'static str {
                match self {
                    #(#as_ref_str)*
                }
            }
        }

        impl AsRef<str> for #ident {
            fn as_ref(&self) -> &str {
                self.as_static_str()
            }
        }

        impl std::fmt::Display for #ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.as_ref())
            }
        }

        impl std::convert::TryFrom<usize> for #ident {
            type Error = &'static str;

            fn try_from(id: usize) -> Result<Self, Self::Error> {
                match id {
                    #(#from_id)*
                    _ => Err("Invalid privsep process ID"),
                }
            }
        }
    })
}
