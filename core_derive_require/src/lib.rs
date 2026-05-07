use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, GenericArgument, PathArguments, Type};

/// Helper to check if a type is a Vec<T> or Option<T> and extract the T.
enum TypeWrapper<'a> {
    Vec(&'a Type),
    Option(&'a Type),
    None(&'a Type),
}

fn extract_wrapper(ty: &Type) -> TypeWrapper {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if (ident == "Vec" || ident == "Option") {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        if ident == "Vec" {
                            return TypeWrapper::Vec(inner_ty);
                        } else {
                            return TypeWrapper::Option(inner_ty);
                        }
                    }
                }
            }
        }
    }
    TypeWrapper::None(ty)
}

/// Helper to parse `#[require(filter = |x| ...)]`
fn extract_filter_expr(attrs: &[syn::Attribute]) -> Option<Expr> {
    for attr in attrs {
        if attr.path().is_ident("require") {
            let mut filter_expr = None;
            // Parse the nested meta items: `filter = ...`
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("filter") {
                    // Extract the value as an Expression (this handles closures natively!)
                    let value = meta.value()?;
                    filter_expr = Some(value.parse::<Expr>()?);
                    Ok(())
                } else {
                    Err(meta.error("unsupported attribute"))
                }
            });
            if filter_expr.is_some() {
                return filter_expr;
            }
        }
    }
    None
}

#[proc_macro_derive(Require, attributes(require))]
pub fn derive_require(input: TokenStream) -> TokenStream {
    // Parse input tokens into a syntax tree
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    
    // Support generics
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the body of the `require` function based on the struct's fields
    let require_body = match input.data {
        // Structs with named fields: struct Foo { bar: Bar }
        Data::Struct(ref data_struct) => match data_struct.fields {
            Fields::Named(ref fields) => {
                let mut field_inits = Vec::new();
                let mut field_names = Vec::new();

                for field in fields.named.iter() {
                    let field_name = field.ident.as_ref().unwrap();
                    let filter = extract_filter_expr(&field.attrs);
                    let wrapper = extract_wrapper(&field.ty);

                    let init_tokens = match (wrapper, filter) {
                        // 1. Bare Type, NO filter
                        (TypeWrapper::None(inner), None) => quote! {
                            <#inner>::require(assets)?
                        },
                        // 2. Bare Type, WITH filter
                        (TypeWrapper::None(inner), Some(f)) => quote! {
                            <#inner>::require_iter(assets)?
                                .find(#f)
                                .ok_or_else(|| MeetRequirementError::DependencyNotAvailable(
                                    std::any::type_name::<#inner>().to_string()
                                ))?
                        },
                        // 3. Option<T>, NO filter (Swallows error)
                        (TypeWrapper::Option(inner), None) => quote! {
                            <#inner>::require(assets).ok()
                        },
                        // 4. Option<T>, WITH filter (Swallows error, applies filter)
                        (TypeWrapper::Option(inner), Some(f)) => quote! {
                            <#inner>::require_iter(assets)
                                .map(|mut it| it.find(#f))
                                .unwrap_or(None)
                        },
                        // 5. Vec<T>, NO filter (Bubbles error, collects all)
                        (TypeWrapper::Vec(inner), None) => quote! {
                            <#inner>::require_iter(assets)?.collect()
                        },
                        // 6. Vec<T>, WITH filter (Bubbles error, collects filtered)
                        (TypeWrapper::Vec(inner), Some(f)) => quote! {
                            <#inner>::require_iter(assets)?.filter(#f).collect()
                        },
                    };

                    field_names.push(field_name);
                    field_inits.push(quote! { let #field_name = #init_tokens; });
                }

                quote! {
                    #( #field_inits )*
                    Ok(std::iter::once(Self {
                        #( #field_names ),*
                    }))
                }
            }
            // Tuple structs: struct Foo(Bar, Baz)
            Fields::Unnamed(ref _fields) => {
                // TODO: Implement Fields::Unnamed similarly if tuple structs need attributes
                quote! {
                    compile_error!("Currently only named structs are supported in this POC.");
                }
                // let field_types = fields.unnamed.iter().map(|f| &f.ty);
                
                // quote! {
                //     Ok(std::iter::once(Self(
                //         #(
                //             <#field_types>::require(assets)?
                //         ),*
                //     )))
                // }
            }
            // Unit structs: struct Bar;
            Fields::Unit => {
                quote! {
                    Ok(std::iter::once(Self))
                }
            }
        },
        _ => quote! { compile_error!("Require can only be derived for structs"); },
    };

    let expanded = quote! {
        impl #impl_generics Require for #name #ty_generics #where_clause {
            fn require_iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, MeetRequirementError> {
                #require_body
            }
        }
    };

    TokenStream::from(expanded)
}