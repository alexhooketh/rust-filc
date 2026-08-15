use std::collections::HashSet;
use std::fmt::{self, Write as _};

use quote::ToTokens as _;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, ForeignItem, ForeignItemFn, Item, ItemForeignMod, Lit, Meta, Pat,
    PathArguments, ReturnType, Token,
};

#[derive(Debug)]
pub(crate) struct Schema {
    pub bridge: Bridge,
    pub handles: Vec<Handle>,
    pub functions: Vec<Function>,
}

#[derive(Debug)]
pub(crate) struct Bridge {
    pub name: String,
    pub header: String,
    pub sources: Vec<String>,
    pub includes: Vec<String>,
    pub max_frame_bytes: u32,
}

const fn default_max_frame_bytes() -> u32 {
    16 * 1024 * 1024
}

#[derive(Debug)]
pub(crate) struct Handle {
    pub name: String,
    pub rust_name: String,
    pub c_type: String,
    pub drop: Option<DropSpec>,
}

#[derive(Debug)]
pub(crate) struct DropSpec {
    pub name: String,
    pub visibility: String,
    pub symbol: String,
}

#[derive(Debug)]
pub(crate) struct Function {
    pub name: String,
    pub visibility: String,
    pub symbol: String,
    pub params: Vec<Param>,
    pub result: ResultSpec,
}

#[derive(Debug)]
pub(crate) struct Param {
    pub name: String,
    pub ty: String,
}

#[derive(Debug)]
pub(crate) struct ResultSpec {
    pub ty: String,
    pub free: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Type {
    Bool,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
    Bytes,
    Handle(String),
    Void,
}

impl Type {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "bool" => Ok(Self::Bool),
            "i32" => Ok(Self::I32),
            "u32" => Ok(Self::U32),
            "i64" => Ok(Self::I64),
            "u64" => Ok(Self::U64),
            "f32" => Ok(Self::F32),
            "f64" => Ok(Self::F64),
            "string" => Ok(Self::String),
            "bytes" => Ok(Self::Bytes),
            "void" => Ok(Self::Void),
            _ => value
                .strip_prefix("handle:")
                .filter(|name| !name.is_empty())
                .map(|name| Self::Handle(name.to_owned()))
                .ok_or_else(|| format!("unsupported boundary type `{value}`")),
        }
    }

    pub const fn rust_argument(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "&str",
            Self::Bytes => "&[u8]",
            Self::Handle(_) | Self::Void => "",
        }
    }

    pub const fn rust_result(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "String",
            Self::Bytes => "Vec<u8>",
            Self::Void => "()",
            Self::Handle(_) => "",
        }
    }

    pub const fn codec_method(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::I64 => "i64",
            Self::U64 | Self::Handle(_) => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "string",
            Self::Bytes => "bytes",
            Self::Void => "void",
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handle(name) => write!(formatter, "handle:{name}"),
            _ => formatter.write_str(self.codec_method()),
        }
    }
}

impl Schema {
    pub fn parse(source: &str) -> Result<Self, String> {
        let file = syn::parse_file(source).map_err(|error| error.to_string())?;
        let mut declarations = Vec::new();
        collect_bridges(&file.items, &mut declarations);
        let [foreign] = declarations.as_slice() else {
            return Err(format!(
                "expected exactly one `#[filc::bridge]` block, found {}",
                declarations.len()
            ));
        };
        Self::from_foreign(foreign)
    }

    fn from_foreign(foreign: &ItemForeignMod) -> Result<Self, String> {
        if foreign.unsafety.is_none() {
            return Err("the bridge must be declared with `unsafe extern`".into());
        }
        let abi = foreign
            .abi
            .name
            .as_ref()
            .map(syn::LitStr::value)
            .unwrap_or_default();
        if !matches!(abi.as_str(), "Fil-C" | "fil-c") {
            return Err(
                "the bridge ABI must be `Fil-C` (lowercase `fil-c` is also accepted)".into(),
            );
        }

        let bridge_attribute = foreign
            .attrs
            .iter()
            .find(|attribute| is_path(attribute, &["filc", "bridge"]))
            .expect("collected bridge has its attribute");
        let bridge = parse_bridge(bridge_attribute)?;
        let foreign_functions = foreign
            .items
            .iter()
            .map(|item| {
                let ForeignItem::Fn(function) = item else {
                    return Err("Fil-C bridge blocks currently support functions only".to_owned());
                };
                Ok(function)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut handles = Vec::new();
        for function in &foreign_functions {
            collect_pointer_handles(function, &mut handles)?;
        }
        let mut functions = Vec::new();
        for function in foreign_functions {
            let parsed = parse_function(function)?;
            if is_path_attribute(function, &["filc", "drop"])? {
                attach_drop(&mut handles, parsed)?;
            } else {
                functions.push(parsed);
            }
        }

        let schema = Self {
            bridge,
            handles,
            functions,
        };
        schema.validate()?;
        Ok(schema)
    }

    pub fn canonical(&self) -> String {
        let mut output = String::new();
        writeln!(output, "bridge:{}", self.bridge.name).unwrap();
        writeln!(output, "max_frame_bytes:{}", self.bridge.max_frame_bytes).unwrap();
        for handle in &self.handles {
            writeln!(
                output,
                "handle:{}:{}:{}",
                handle.name,
                handle.c_type,
                handle.drop.as_ref().map_or("", |drop| drop.symbol.as_str())
            )
            .unwrap();
        }
        for function in &self.functions {
            write!(output, "fn:{}:{}(", function.name, function.symbol).unwrap();
            for param in &function.params {
                write!(output, "{}:{},", param.name, param.ty).unwrap();
            }
            writeln!(
                output,
                ")->{}:free={}",
                function.result.ty,
                function.result.free.as_deref().unwrap_or("")
            )
            .unwrap();
        }
        output
    }

    pub fn handle(&self, name: &str) -> &Handle {
        self.handles
            .iter()
            .find(|handle| handle.name == name)
            .expect("validated handle reference")
    }

    fn validate(&self) -> Result<(), String> {
        identifier(&self.bridge.name, "bridge name")?;
        if self.bridge.header.is_empty() || self.bridge.header.contains(['"', '\n', '\r']) {
            return Err("bridge header must be a nonempty quoted-include path".into());
        }
        if self.bridge.sources.is_empty() {
            return Err("a bridge must declare at least one C source".into());
        }
        if self.bridge.max_frame_bytes < 1024 {
            return Err("bridge max_frame_bytes must be at least 1024".into());
        }
        if self.functions.is_empty() {
            return Err("a bridge must declare at least one function".into());
        }

        let mut handle_names = HashSet::new();
        for handle in &self.handles {
            identifier(&handle.name, "handle name")?;
            identifier(&handle.rust_name, "generated Rust handle name")?;
            if let Some(drop) = &handle.drop {
                identifier(&drop.name, "handle drop function")?;
                identifier(&drop.symbol, "handle drop symbol")?;
            }
            if !handle.c_type.trim_end().ends_with('*') || handle.c_type.contains(['\n', '\r', ';'])
            {
                return Err(format!("handle `{}` has an invalid C type", handle.name));
            }
            if !handle_names.insert(handle.name.as_str()) {
                return Err(format!("duplicate handle `{}`", handle.name));
            }
        }

        let mut function_names = HashSet::new();
        for function in &self.functions {
            identifier(&function.name, "function name")?;
            identifier(&function.symbol, "function symbol")?;
            if !function_names.insert(function.name.as_str()) {
                return Err(format!("duplicate function `{}`", function.name));
            }
            let mut param_names = HashSet::new();
            for param in &function.params {
                identifier(&param.name, "parameter name")?;
                if !param_names.insert(param.name.as_str()) {
                    return Err(format!(
                        "duplicate parameter `{}` in `{}`",
                        param.name, function.name
                    ));
                }
                let ty = Type::parse(&param.ty)?;
                if ty == Type::Void {
                    return Err(format!("parameter `{}` cannot have type void", param.name));
                }
                validate_handle_type(&ty, &handle_names)?;
            }
            let result = Type::parse(&function.result.ty)?;
            validate_handle_type(&result, &handle_names)?;
            if let Some(free) = &function.result.free {
                identifier(free, "result free symbol")?;
                if !matches!(result, Type::String | Type::Bytes) {
                    return Err(format!(
                        "function `{}` only permits `#[filc::free]` for String or Vec<u8> results",
                        function.name
                    ));
                }
            }
        }
        Ok(())
    }
}

fn collect_bridges<'a>(items: &'a [Item], output: &mut Vec<&'a ItemForeignMod>) {
    for item in items {
        match item {
            Item::ForeignMod(foreign)
                if foreign
                    .attrs
                    .iter()
                    .any(|attribute| is_path(attribute, &["filc", "bridge"])) =>
            {
                output.push(foreign);
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_bridges(items, output);
                }
            }
            _ => {}
        }
    }
}

fn parse_bridge(attribute: &Attribute) -> Result<Bridge, String> {
    let arguments = attribute
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|error| error.to_string())?;
    let name = required_string(&arguments, "name")?;
    let header = required_string(&arguments, "header")?;
    let sources = required_strings(&arguments, "sources")?;
    let includes = optional_strings(&arguments, "includes")?.unwrap_or_default();
    let max_frame_bytes =
        optional_u32(&arguments, "max_frame_bytes")?.unwrap_or_else(default_max_frame_bytes);
    reject_unknown(
        &arguments,
        &["name", "header", "sources", "includes", "max_frame_bytes"],
    )?;
    Ok(Bridge {
        name,
        header,
        sources,
        includes,
        max_frame_bytes,
    })
}

fn parse_function(function: &ForeignItemFn) -> Result<Function, String> {
    validate_function_attributes(function)?;
    if function.sig.unsafety.is_some() {
        return Err(format!(
            "function `{}` is explicitly unsafe; the supported bridge generates safe calls only",
            function.sig.ident
        ));
    }
    if function.sig.variadic.is_some() || !function.sig.generics.params.is_empty() {
        return Err(format!(
            "function `{}` cannot be variadic or generic",
            function.sig.ident
        ));
    }
    let mut params = Vec::new();
    for input in &function.sig.inputs {
        let FnArg::Typed(input) = input else {
            return Err("Fil-C declarations cannot have a receiver".into());
        };
        let Pat::Ident(pattern) = input.pat.as_ref() else {
            return Err(format!(
                "function `{}` parameters must be identifiers",
                function.sig.ident
            ));
        };
        params.push(Param {
            name: pattern.ident.to_string(),
            ty: parse_type(&input.ty, false)?,
        });
    }
    let result_ty = match &function.sig.output {
        ReturnType::Default => "void".into(),
        ReturnType::Type(_, ty) => parse_type(ty, true)?,
    };
    let name = function.sig.ident.to_string();
    let symbol = attribute_string(&function.attrs, &["link_name"])?.unwrap_or_else(|| name.clone());
    let free = attribute_string(&function.attrs, &["filc", "free"])?;
    Ok(Function {
        name,
        visibility: function.vis.to_token_stream().to_string(),
        symbol,
        params,
        result: ResultSpec {
            ty: result_ty,
            free,
        },
    })
}

fn validate_function_attributes(function: &ForeignItemFn) -> Result<(), String> {
    for attribute in &function.attrs {
        let supported = is_path(attribute, &["doc"])
            || is_path(attribute, &["link_name"])
            || is_path(attribute, &["filc", "free"])
            || is_path(attribute, &["filc", "drop"]);
        if !supported {
            return Err(format!(
                "function `{}` has unsupported attribute `{}`",
                function.sig.ident,
                attribute.path().to_token_stream()
            ));
        }
    }
    Ok(())
}

fn collect_pointer_handles(
    function: &ForeignItemFn,
    handles: &mut Vec<Handle>,
) -> Result<(), String> {
    for input in &function.sig.inputs {
        if let FnArg::Typed(input) = input {
            collect_pointer_handle(&input.ty, handles)?;
        }
    }
    if let ReturnType::Type(_, result) = &function.sig.output {
        collect_pointer_handle(result, handles)?;
    }
    Ok(())
}

fn collect_pointer_handle(ty: &syn::Type, handles: &mut Vec<Handle>) -> Result<(), String> {
    let syn::Type::Ptr(pointer) = ty else {
        return Ok(());
    };
    let Some(name) = path_ident(&pointer.elem) else {
        return Err(format!(
            "opaque Fil-C pointer `{}` must point to one named C type",
            ty.to_token_stream()
        ));
    };
    let name = name.to_string();
    if handles.iter().all(|handle| handle.name != name) {
        handles.push(Handle {
            rust_name: pascal(&name),
            c_type: format!("{name} *"),
            name,
            drop: None,
        });
    }
    Ok(())
}

fn attach_drop(handles: &mut [Handle], function: Function) -> Result<(), String> {
    if function.result.ty != "void" || function.params.len() != 1 {
        return Err(format!(
            "#[filc::drop] function `{}` must take exactly one opaque pointer and return void",
            function.name
        ));
    }
    let Type::Handle(handle_name) = Type::parse(&function.params[0].ty)? else {
        return Err(format!(
            "#[filc::drop] function `{}` must take an opaque pointer",
            function.name
        ));
    };
    let handle = handles
        .iter_mut()
        .find(|handle| handle.name == handle_name)
        .expect("pointer handles were collected before drop metadata");
    if handle.drop.is_some() {
        return Err(format!("duplicate destructor for `{handle_name}`"));
    }
    handle.drop = Some(DropSpec {
        name: function.name,
        visibility: function.visibility,
        symbol: function.symbol,
    });
    Ok(())
}

fn parse_type(ty: &syn::Type, result: bool) -> Result<String, String> {
    Ok(type_from_syn(ty, result)?.to_string())
}

fn type_from_syn(ty: &syn::Type, result: bool) -> Result<Type, String> {
    if let syn::Type::Tuple(tuple) = ty
        && tuple.elems.is_empty()
    {
        return Ok(Type::Void);
    }
    if let syn::Type::Reference(reference) = ty {
        if result || reference.mutability.is_some() {
            return Err("results and mutable references cannot cross the Fil-C boundary".into());
        }
        if let syn::Type::Slice(slice) = reference.elem.as_ref()
            && path_ident(slice.elem.as_ref()).is_some_and(|name| name == "u8")
        {
            return Ok(Type::Bytes);
        }
        if path_ident(reference.elem.as_ref()).is_some_and(|name| name == "str") {
            return Ok(Type::String);
        }
        return Err("only borrowed str and [u8] values cross the Fil-C boundary".into());
    }
    if let syn::Type::Ptr(pointer) = ty {
        let Some(name) = path_ident(&pointer.elem) else {
            return Err(format!(
                "opaque Fil-C pointer `{}` must point to one named C type",
                ty.to_token_stream()
            ));
        };
        return Ok(Type::Handle(name.to_string()));
    }
    let Some(name) = path_ident(ty) else {
        return Err(format!(
            "unsupported boundary type `{}`",
            ty.to_token_stream()
        ));
    };
    let name = name.to_string();
    match name.as_str() {
        "bool" => Ok(Type::Bool),
        "i32" => Ok(Type::I32),
        "u32" => Ok(Type::U32),
        "i64" => Ok(Type::I64),
        "u64" => Ok(Type::U64),
        "f32" => Ok(Type::F32),
        "f64" => Ok(Type::F64),
        "String" if result => Ok(Type::String),
        "Vec" if result && is_vec_u8(ty) => Ok(Type::Bytes),
        "String" | "Vec" => Err(format!("`{name}` is only supported as an owned result")),
        _ => Err(format!(
            "unsupported boundary type `{name}`; opaque C values must be written as pointers"
        )),
    }
}

fn is_path_attribute(function: &ForeignItemFn, path: &[&str]) -> Result<bool, String> {
    let matching = function
        .attrs
        .iter()
        .filter(|attribute| is_path(attribute, path))
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(format!("duplicate `{}` attribute", path.join("::")));
    }
    if let Some(attribute) = matching.first()
        && !matches!(&attribute.meta, Meta::Path(_))
    {
        return Err(format!(
            "attribute `{}` takes no arguments",
            path.join("::")
        ));
    }
    Ok(!matching.is_empty())
}

fn path_ident(ty: &syn::Type) -> Option<&syn::Ident> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() || path.path.segments.len() != 1 {
        return None;
    }
    Some(&path.path.segments.first()?.ident)
}

fn is_vec_u8(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    matches!(
        arguments.args.first(),
        Some(syn::GenericArgument::Type(inner))
            if path_ident(inner).is_some_and(|name| name == "u8")
    )
}

fn attribute_string(attrs: &[Attribute], path: &[&str]) -> Result<Option<String>, String> {
    let mut result = None;
    for attribute in attrs.iter().filter(|attribute| is_path(attribute, path)) {
        let value = match &attribute.meta {
            Meta::NameValue(name_value) => expression_string(&name_value.value)?,
            Meta::List(_) => attribute
                .parse_args::<syn::LitStr>()
                .map_err(|error| error.to_string())?
                .value(),
            Meta::Path(_) => {
                return Err(format!("attribute `{}` requires a string", path.join("::")));
            }
        };
        if result.replace(value).is_some() {
            return Err(format!("duplicate `{}` attribute", path.join("::")));
        }
    }
    Ok(result)
}

fn required_string(arguments: &Punctuated<Meta, Token![,]>, name: &str) -> Result<String, String> {
    optional_string(arguments, name)?.ok_or_else(|| format!("missing `{name} = \"...\"`"))
}

fn optional_string(
    arguments: &Punctuated<Meta, Token![,]>,
    name: &str,
) -> Result<Option<String>, String> {
    let mut result = None;
    for argument in arguments {
        let Meta::NameValue(value) = argument else {
            continue;
        };
        if value.path.is_ident(name) {
            let parsed = expression_string(&value.value)?;
            if result.replace(parsed).is_some() {
                return Err(format!("duplicate `{name}` argument"));
            }
        }
    }
    Ok(result)
}

fn required_strings(
    arguments: &Punctuated<Meta, Token![,]>,
    name: &str,
) -> Result<Vec<String>, String> {
    optional_strings(arguments, name)?.ok_or_else(|| format!("missing `{name} = [...]`"))
}

fn optional_strings(
    arguments: &Punctuated<Meta, Token![,]>,
    name: &str,
) -> Result<Option<Vec<String>>, String> {
    let mut result = None;
    for argument in arguments {
        let Meta::NameValue(value) = argument else {
            continue;
        };
        if !value.path.is_ident(name) {
            continue;
        }
        let Expr::Array(array) = &value.value else {
            return Err(format!("`{name}` must be an array of strings"));
        };
        let parsed = array
            .elems
            .iter()
            .map(expression_string)
            .collect::<Result<Vec<_>, _>>()?;
        if result.replace(parsed).is_some() {
            return Err(format!("duplicate `{name}` argument"));
        }
    }
    Ok(result)
}

fn optional_u32(
    arguments: &Punctuated<Meta, Token![,]>,
    name: &str,
) -> Result<Option<u32>, String> {
    let mut result = None;
    for argument in arguments {
        let Meta::NameValue(value) = argument else {
            continue;
        };
        if !value.path.is_ident(name) {
            continue;
        }
        let Expr::Lit(expression) = &value.value else {
            return Err(format!("`{name}` must be an integer literal"));
        };
        let Lit::Int(integer) = &expression.lit else {
            return Err(format!("`{name}` must be an integer literal"));
        };
        let parsed = integer
            .base10_parse::<u32>()
            .map_err(|error| error.to_string())?;
        if result.replace(parsed).is_some() {
            return Err(format!("duplicate `{name}` argument"));
        }
    }
    Ok(result)
}

fn reject_unknown(arguments: &Punctuated<Meta, Token![,]>, allowed: &[&str]) -> Result<(), String> {
    for argument in arguments {
        let Some(identifier) = argument.path().get_ident() else {
            return Err("bridge arguments must use simple names".into());
        };
        if !allowed.iter().any(|allowed| identifier == allowed) {
            return Err(format!("unknown bridge argument `{identifier}`"));
        }
    }
    Ok(())
}

fn expression_string(expression: &Expr) -> Result<String, String> {
    let Expr::Lit(expression) = expression else {
        return Err("expected a string literal".into());
    };
    let Lit::Str(value) = &expression.lit else {
        return Err("expected a string literal".into());
    };
    Ok(value.value())
}

fn is_path(attribute: &Attribute, expected: &[&str]) -> bool {
    attribute.path().segments.len() == expected.len()
        && attribute
            .path()
            .segments
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.ident == expected)
}

fn validate_handle_type(ty: &Type, handles: &HashSet<&str>) -> Result<(), String> {
    if let Type::Handle(name) = ty
        && !handles.contains(name.as_str())
    {
        return Err(format!("unknown opaque handle `{name}`"));
    }
    Ok(())
}

fn identifier(value: &str, kind: &str) -> Result<(), String> {
    let mut chars = value.chars();
    let valid_first = chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    if !valid_first || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(format!("{kind} `{value}` is not an ASCII identifier"));
    }
    Ok(())
}

pub(crate) fn shouty(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() && index != 0 {
            result.push('_');
        }
        result.push(character.to_ascii_uppercase());
    }
    result
}

fn pascal(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut uppercase = true;
    for character in value.chars() {
        if character == '_' || character == '-' {
            uppercase = true;
        } else if uppercase {
            result.push(character.to_ascii_uppercase());
            uppercase = false;
        } else {
            result.push(character);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Schema, shouty};

    const DECLARATION: &str = r#"
        pub mod demo {
            #[filc::bridge(
                name = "demo_api",
                header = "demo.h",
                sources = ["demo.c"],
                includes = ["."],
            )]
            unsafe extern "Fil-C" {
                pub fn counter_new(value: i32) -> *mut counter_t;
                pub fn counter_add(counter: *mut counter_t, delta: i32) -> i32;
                #[filc::drop]
                pub fn counter_drop(counter: *mut counter_t);
                #[link_name = "legacy_greet"]
                #[filc::free("release_string")]
                pub fn greet(name: &str) -> String;
            }
        }
    "#;

    #[test]
    fn parses_rust_shaped_bridge_declarations() {
        let schema = Schema::parse(DECLARATION).unwrap();
        assert_eq!(schema.bridge.name, "demo_api");
        assert_eq!(schema.bridge.sources, ["demo.c"]);
        assert_eq!(schema.bridge.max_frame_bytes, 16 * 1024 * 1024);
        assert_eq!(schema.handles[0].name, "counter_t");
        assert_eq!(schema.handles[0].rust_name, "CounterT");
        assert_eq!(
            schema.handles[0].drop.as_ref().unwrap().symbol,
            "counter_drop"
        );
        assert_eq!(schema.functions[1].params[0].ty, "handle:counter_t");
        assert_eq!(schema.functions[2].symbol, "legacy_greet");
        assert_eq!(schema.functions[2].result.ty, "string");
    }

    #[test]
    fn canonical_schema_ignores_formatting() {
        let first = Schema::parse(DECLARATION).unwrap().canonical();
        let compact = DECLARATION.replace("        ", "");
        let second = Schema::parse(&compact).unwrap().canonical();
        assert_eq!(first, second);
    }

    #[test]
    fn accepts_lowercase_fil_c_as_an_alias() {
        let lowercase = DECLARATION.replace("extern \"Fil-C\"", "extern \"fil-c\"");
        Schema::parse(&lowercase).unwrap();
    }

    #[test]
    fn rejects_non_pointer_opaque_types_and_invalid_destructors() {
        let invalid = DECLARATION.replace("counter: *mut counter_t", "counter: counter_t");
        assert!(Schema::parse(&invalid).is_err());
        let invalid_drop = DECLARATION.replace(
            "pub fn counter_drop(counter: *mut counter_t);",
            "pub fn counter_drop(counter: *mut counter_t) -> i32;",
        );
        assert!(Schema::parse(&invalid_drop).is_err());
    }

    #[test]
    fn names_are_deterministic() {
        assert_eq!(shouty("CounterDemo"), "COUNTER_DEMO");
        assert_eq!(shouty("counter_demo"), "COUNTER_DEMO");
    }
}
