use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

#[proc_macro_derive(Require)]
pub fn derive_require(input: TokenStream) -> TokenStream {
    // Parse input tokens into a syntax tree
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    
    // Support generics
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the body of the `require` function based on the struct's fields
    let require_body = match input.data {
        Data::Struct(ref data_struct) => match data_struct.fields {
            // Structs with named fields: struct Foo { bar: Bar }
            Fields::Named(ref fields) => {
                let field_names = fields.named.iter().map(|f| &f.ident);
                let field_types = fields.named.iter().map(|f| &f.ty);
                
                // We clone the iterator for the struct initialization
                let field_names_init = field_names.clone();

                quote! {
                    #(
                        let #field_names = <#field_types>::require(assets)?;
                    )*
                    // Wrap the constructed struct in an iterator of length 1
                    Ok(std::iter::once(Self {
                        #( #field_names_init ),*
                    }))
                }
            }
            // Tuple structs: struct Foo(Bar, Baz)
            Fields::Unnamed(ref fields) => {
                let field_types = fields.unnamed.iter().map(|f| &f.ty);
                
                quote! {
                    Ok(std::iter::once(Self(
                        #(
                            <#field_types>::require(assets)?
                        ),*
                    )))
                }
            }
            // Unit structs: struct Bar;
            Fields::Unit => {
                quote! {
                    Ok(std::iter::once(Self))
                }
            }
        },
        _ => {
            // Return a compile error if someone tries to derive this on an Enum or Union
            return syn::Error::new_spanned(name, "Require can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    // Combine everything into the final trait implementation
    let expanded = quote! {
        impl #impl_generics Require for #name #ty_generics #where_clause {
            fn require_iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, MeetRequirementError> {
                #require_body
            }
        }
    };

    TokenStream::from(expanded)
}