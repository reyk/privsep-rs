//! Helper macros for `privep-rs`

use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{
    parse::Parse, parse_macro_input, Attribute, Error, ItemEnum, Lit, LitStr, Meta, MetaNameValue,
    Path,
};

#[proc_macro_derive(Privsep, attributes(main_path, username, disable_privdrop))]
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
    let mut const_as_array = vec![];
    let mut const_id = vec![];
    let mut const_ids = vec![];
    let mut from_id = vec![];

    let doc = attrs.iter().filter(|a| a.path.is_ident("doc"));
    let main_path: Path = parse_attribute_type(&attrs, "main_path", "parent::main")?;

    let disable_privdrop = attrs.iter().any(|a| a.path.is_ident("disable_privdrop"));
    let username = if let Some(username) = parse_attribute_value(&attrs, "username")? {
        username
    } else if disable_privdrop {
        LitStr::new("", Span::call_site())
    } else {
        return Err(Error::new(
            Span::call_site(),
            "`Privsep` requires `username` attribute",
        ));
    };
    let options = quote! {
        privsep::process::Options {
            disable_privdrop: #disable_privdrop,
            ..Default::default()
        }
    };

    for (id, variant) in item.variants.iter().enumerate() {
        let child_doc = variant.attrs.iter().filter(|a| a.path.is_ident("doc"));
        let ident = &variant.ident;
        let name = ident.to_string().to_case(Case::Kebab);
        let name_snake = ident.to_string().to_case(Case::Snake);
        let name_upper = ident.to_string().to_case(Case::UpperSnake);
        let id_name = Ident::new(&(name_upper + "_ID"), Span::call_site());
        let child_path: Path =
            parse_attribute_type(&variant.attrs, "main_path", &(name_snake + "::main"))?;

        let child_username =
            parse_attribute_value(&variant.attrs, "username")?.unwrap_or_else(|| username.clone());
        let child_disable_privdrop =
            disable_privdrop || attrs.iter().any(|a| a.path.is_ident("disable_privdrop"));
        let child_options = quote! {
            privsep::process::Options {
                disable_privdrop: #child_disable_privdrop,
                username: #child_username.into(),
                ..Default::default()
            }
        };

        const_as_array.push(quote! {
            Process { name: #name },
        });

        const_id.push(quote! {
            #(#child_doc)*
            pub const #id_name: usize = #id;
        });

        const_ids.push(quote! {
            #id,
        });

        as_ref_str.push(quote! {
            Self::#ident => #name,
        });

        child_main.push(quote! {
            #name => {
                let child = Child::new(#name, &#child_options).await?;
                #child_path(child).await
            }
        });

        from_id.push(quote! {
            #id => Ok(Self::#ident),
        });
    }
    let array_len = const_as_array.len();

    Ok(quote! {
        #(#doc)*
        impl #ident {
            #(#const_id)*

            #[doc = "IDs of all child processes."]
            pub const PROCESS_IDS: [usize; #array_len] = [#(#const_ids)*];

            #[doc = "Return processes as const list."]
            pub const fn as_array() -> [privsep::process::Process; #array_len] {
                use privsep::process::Process;
                [
                    #(#const_as_array)*
                ]
            }

            #[doc = "Start parent or child process."]
            pub async fn main() -> Result<(), privsep::Error> {
                use privsep::process::{Child, Parent};
                let name = std::env::args().next().unwrap_or_default();
                match name.as_ref() {
                    #(#child_main)*
                    _ => {
                        let parent = Parent::new(Self::as_array(), &#options).await?;
                        #main_path(parent).await
                    }
                }
            }
        }

        impl AsRef<str> for #ident {
            fn as_ref(&self) -> &str {
                match self {
                    #(#as_ref_str)*
                }
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
