use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, GenericArgument, PathArguments, Type};

enum TypeWrapper<'a> {
    Vec(&'a Type),
    Option(&'a Type),
    None(&'a Type),
}

fn extract_wrapper(ty: &'_ Type) -> TypeWrapper<'_> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if ident == "Vec" || ident == "Option" {
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

struct FieldConfig {
    filter: Option<Expr>,
    each: bool,
}

fn parse_require_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut config = FieldConfig { filter: None, each: false };
    for attr in attrs {
        if attr.path().is_ident("require") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("filter") {
                    config.filter = Some(meta.value()?.parse()?);
                    Ok(())
                } else if meta.path.is_ident("each") {
                    config.each = true;
                    Ok(())
                } else {
                    Err(meta.error("unsupported require attribute"))
                }
            })?;
        }
    }
    Ok(config)
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
        Data::Struct(ref data_struct) => match data_struct.fields {
            // Structs with named fields: struct Foo { bar: Bar }
            Fields::Named(ref fields) => {
                let mut field_names = Vec::new();
                let mut non_each_inits = Vec::new();
                let mut each_loops = Vec::new();

                for field in fields.named.iter() {
                    let field_name = field.ident.as_ref().unwrap();
                    let attrs = parse_require_attrs(&field.attrs).unwrap_or_else(|e| {
                        panic!("Failed to parse attributes for field '{}': {}", field_name, e)
                    });
                    let wrapper = extract_wrapper(&field.ty);

                    field_names.push(field_name);

                    if attrs.each {
                        let TypeWrapper::None(inner) = wrapper else {
                            return syn::Error::new_spanned(
                                &field.ty, 
                                "`each` cannot be used with Vec or Option"
                            ).to_compile_error().into();
                        };

                        let iter_expr = match attrs.filter {
                            None => quote! { <#inner>::require_iter(assets)? },
                            Some(f) => quote! { <#inner>::require_iter(assets)?.filter(#f) },
                        };
                        
                        each_loops.push((field_name, iter_expr));
                    } else {
                        // Standard field logic
                        let init_tokens = match (wrapper, attrs.filter) {
                            (TypeWrapper::None(inner), None) => quote! { <#inner>::require(assets)? },
                            (TypeWrapper::None(inner), Some(f)) => quote! {
                                <#inner>::require_iter(assets)?
                                    .find(#f)
                                    .ok_or_else(|| MeetRequirementError::DependencyNotAvailable(
                                        std::any::type_name::<#inner>().to_string()
                                    ))?
                            },
                            (TypeWrapper::Option(inner), None) => quote! { <#inner>::require(assets).ok() },
                            (TypeWrapper::Option(inner), Some(f)) => quote! {
                                <#inner>::require_iter(assets).map(|mut it| it.find(#f)).unwrap_or(None)
                            },
                            (TypeWrapper::Vec(inner), None) => quote! { <#inner>::require_iter(assets)?.collect() },
                            (TypeWrapper::Vec(inner), Some(f)) => quote! { <#inner>::require_iter(assets)?.filter(#f).collect() },
                        };

                        non_each_inits.push(quote! { let #field_name = #init_tokens; });
                    }
                }

                // Determine which fields need to be cloned
                let mut struct_inits = Vec::new();
                for field_name in &field_names {
                    let is_each = each_loops.iter().any(|(n, _)| n == field_name);
                    let is_last_each = each_loops.last().map(|(n, _)| n) == Some(field_name);
                    
                    if is_each && !is_last_each {
                        // Outer `each` fields must be cloned for the inner loops!
                        struct_inits.push(quote! { #field_name: #field_name.clone() });
                    } else {
                        // Standard fields and the innermost `each` field do not need cloning
                        struct_inits.push(quote! { #field_name });
                    }
                }

                if each_loops.is_empty() {
                    quote! {
                        #( #non_each_inits )*
                        Ok(std::iter::once(Self {
                            #( #struct_inits ),*
                        }))
                    }
                } else {
                    let mut inner_block = quote! {
                        #( #non_each_inits )*
                        
                        // We now use the clone-aware initializers
                        results.push(Self {
                            #( #struct_inits ),*
                        });
                    };

                    for (name, iter_expr) in each_loops.into_iter().rev() {
                        inner_block = quote! {
                            for #name in #iter_expr {
                                #inner_block
                            }
                        };
                    }

                    quote! {
                        let mut results = std::vec::Vec::new();
                        #inner_block
                        Ok(results.into_iter())
                    }
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