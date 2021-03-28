//! Helper macros for `privep-rs`

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemEnum};

#[proc_macro_derive(Privsep, attributes(unimplemented))]
pub fn derive_privsep(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(item as ItemEnum);

    derive_privsep_enum(input).into()
}

fn derive_privsep_enum(item: ItemEnum) -> TokenStream {
    let ident = item.ident.clone();
    let mut const_as_array = vec![];
    let mut as_ref_str = vec![];
    let mut from_id = vec![];
    let mut to_id = vec![];
    let doc1 = item.attrs.iter().filter(|attr| attr.path.is_ident("doc"));

    for (id, variant) in item.variants.iter().enumerate() {
        let ident = &variant.ident;
        let name = ident.to_string().to_case(Case::Kebab);

        const_as_array.push(quote! {
            Process { name: #name },
        });

        as_ref_str.push(quote! {
            Self::#ident => #name,
        });

        from_id.push(quote! {
            #id => Ok(Self::#ident),
        });

        to_id.push(quote! {
            Self::#ident => #id,
        });
    }
    let array_len = const_as_array.len();

    quote! {
        #(#doc1)*
        impl #ident {
            pub const fn as_array() -> [privsep::process::Process; #array_len] {
                use privsep::process::Process;
                [
                    #(#const_as_array)*
                ]
            }
        }

        impl AsRef<str> for #ident {
            fn as_ref(&self) -> &str {
                match self {
                    #(#as_ref_str)*
                }
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

        impl std::ops::Deref for #ident {
            type Target = usize;

            fn deref(&self) -> &Self::Target {
                &match self {
                    #(#to_id)*
                }
            }
        }
    }
}
