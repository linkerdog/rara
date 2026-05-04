use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, ItemImpl, Result, Token, braced, parse_macro_input, parse_quote};

struct ToolSpecArgs {
    name: Expr,
    description: Expr,
    input_schema: TokenStream2,
}

impl Parse for ToolSpecArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut description = None;
        let mut input_schema = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => {
                    reject_duplicate(&name, &key)?;
                    name = Some(input.parse()?);
                }
                "description" => {
                    reject_duplicate(&description, &key)?;
                    description = Some(input.parse()?);
                }
                "input_schema" => {
                    reject_duplicate(&input_schema, &key)?;
                    let content;
                    braced!(content in input);
                    input_schema = Some(content.parse()?);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown tool_spec key `{other}`"),
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            name: name.ok_or_else(|| input.error("missing `name`"))?,
            description: description.ok_or_else(|| input.error("missing `description`"))?,
            input_schema: input_schema.ok_or_else(|| input.error("missing `input_schema`"))?,
        })
    }
}

fn reject_duplicate<T>(slot: &Option<T>, key: &Ident) -> Result<()> {
    if slot.is_some() {
        Err(syn::Error::new(
            key.span(),
            format!("duplicate `{}` key", key),
        ))
    } else {
        Ok(())
    }
}

#[proc_macro_attribute]
pub fn tool_spec(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ToolSpecArgs);
    let mut item = parse_macro_input!(item as ItemImpl);
    let name = args.name;
    let description = args.description;
    let input_schema = args.input_schema;

    item.items.insert(
        0,
        parse_quote! {
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({ #input_schema })
            }
        },
    );
    item.items.insert(
        0,
        parse_quote! {
            fn description(&self) -> &str {
                #description
            }
        },
    );
    item.items.insert(
        0,
        parse_quote! {
            fn name(&self) -> &str {
                #name
            }
        },
    );

    quote!(#item).into()
}

#[cfg(test)]
mod tests {
    use super::ToolSpecArgs;

    #[test]
    fn rejects_duplicate_keys() {
        let error = match syn::parse_str::<ToolSpecArgs>(
            r#"name = "first", name = "second", description = "desc", input_schema = {}"#,
        ) {
            Ok(_) => panic!("duplicate name should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("duplicate `name` key"));
    }

    #[test]
    fn rejects_missing_commas_between_keys() {
        let error = match syn::parse_str::<ToolSpecArgs>(
            r#"name = "tool" description = "desc", input_schema = {}"#,
        ) {
            Ok(_) => panic!("missing comma should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("expected `,`"));
    }
}
