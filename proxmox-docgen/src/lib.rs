use serde_json::{Value, json};

use proxmox_router::{ApiAccess, ApiHandler, ApiMethod, Permission, Router, SubRoute};
use proxmox_schema::format::get_property_string_type_text;
use proxmox_schema::{ApiStringFormat, ObjectSchemaType, Schema};

/// Generate a `sede_json::Value` that represents an API in a tree-like structure.
///
/// - `router`: Specifies to root `Router` of the API that should be dumped.
/// - `path`: Defines the base path that will considered as the root of this API (sub-)tree. If
///   `"."` is used, the tree will be considered as starting from the main root (`"/"`).
/// - `privileges`: A slice of tuples that will be used to translate the internal representation of
///   a privilege as a `u64` to it's human readable name.
pub fn generate_api_tree(router: &Router, path: &str, privileges: &[(&str, u64)]) -> Value {
    let mut data = dump_api_schema(router, path, privileges);
    data["expanded"] = true.into();
    data
}

fn dump_schema(schema: &Schema) -> Value {
    let mut data;

    match schema {
        Schema::Null => {
            data = json!({
                "type": "null",
            });
        }
        Schema::Boolean(boolean_schema) => {
            data = json!({
                "type": "boolean",
                "description": boolean_schema.description,
            });
            if let Some(default) = boolean_schema.default {
                data["default"] = default.into();
            }
        }
        Schema::String(string_schema) => {
            data = json!({
                "type": "string",
                "description": string_schema.description,
            });
            if let Some(default) = string_schema.default {
                data["default"] = default.into();
            }
            if let Some(min_length) = string_schema.min_length {
                data["minLength"] = min_length.into();
            }
            if let Some(max_length) = string_schema.max_length {
                data["maxLength"] = max_length.into();
            }
            if let Some(type_text) = string_schema.type_text {
                data["typetext"] = type_text.into();
            }
            match string_schema.format {
                None | Some(ApiStringFormat::VerifyFn(_)) => { /* do nothing */ }
                Some(ApiStringFormat::Pattern(const_regex)) => {
                    data["pattern"] = format!("/{}/", const_regex.regex_string).into();
                }
                Some(ApiStringFormat::Enum(variants)) => {
                    let variants: Vec<String> =
                        variants.iter().map(|e| e.value.to_string()).collect();
                    data["enum"] = serde_json::to_value(variants).unwrap();
                }
                Some(ApiStringFormat::PropertyString(subschema)) => {
                    match subschema {
                        Schema::Object(_) | Schema::Array(_) => {
                            data["format"] = dump_schema(subschema);
                            data["typetext"] = get_property_string_type_text(subschema).into();
                        }
                        _ => { /* do nothing  - should not happen */ }
                    };
                }
            }
        }
        Schema::Integer(integer_schema) => {
            data = json!({
                "type": "integer",
                "description": integer_schema.description,
            });
            if let Some(default) = integer_schema.default {
                data["default"] = default.into();
            }
            if let Some(minimum) = integer_schema.minimum {
                data["minimum"] = minimum.into();
            }
            if let Some(maximum) = integer_schema.maximum {
                data["maximum"] = maximum.into();
            }
        }
        Schema::Number(number_schema) => {
            data = json!({
                "type": "number",
                "description": number_schema.description,
            });
            if let Some(default) = number_schema.default {
                data["default"] = default.into();
            }
            if let Some(minimum) = number_schema.minimum {
                data["minimum"] = minimum.into();
            }
            if let Some(maximum) = number_schema.maximum {
                data["maximum"] = maximum.into();
            }
        }
        Schema::Object(object_schema) => {
            data = dump_object_schema(object_schema);
        }
        Schema::Array(array_schema) => {
            data = json!({
                "type": "array",
                "description": array_schema.description,
                "items": dump_schema(array_schema.items),
            });
            if let Some(min_length) = array_schema.min_length {
                data["minLength"] = min_length.into();
            }
            if let Some(max_length) = array_schema.min_length {
                data["maxLength"] = max_length.into();
            }
        }
        Schema::AllOf(alloff_schema) => {
            data = dump_all_of_schema(alloff_schema);
        }
        Schema::OneOf(schema) => {
            data = dump_one_of_schema(schema);
        }
    };

    data
}

fn dump_parameter_schema(schema: &proxmox_schema::ParameterSchema) -> Value {
    use proxmox_schema::ParameterSchema;
    match schema {
        ParameterSchema::Object(o) => dump_object_schema(o),
        ParameterSchema::AllOf(o) => dump_all_of_schema(o),
        ParameterSchema::OneOf(o) => dump_one_of_schema(o),
    }
}

fn dump_object_schema(schema: &proxmox_schema::ObjectSchema) -> Value {
    let mut properties = json!({});

    for (prop, optional, schema) in schema.properties {
        let mut property = dump_schema(schema);
        if *optional {
            property["optional"] = 1.into();
        }
        properties[prop] = property;
    }

    let mut data = json!({
        "type": "object",
        "description": schema.description,
        "additionalProperties": schema.additional_properties,
        "properties": properties,
    });

    if let Some(default_key) = schema.default_key {
        data["default_key"] = default_key.into();
    }

    data
}

fn dump_all_of_schema(schema: &proxmox_schema::AllOfSchema) -> Value {
    let all_of = schema
        .list
        .iter()
        .copied()
        .map(dump_schema)
        .collect::<Vec<_>>();

    json!({
        "type": "object",
        "description": schema.description,
        "allOf": all_of,
    })
}

fn dump_one_of_schema(schema: &proxmox_schema::OneOfSchema) -> Value {
    let mut one_of = Vec::new();

    for (type_name, schema) in schema.list {
        let mut dump = dump_schema(schema);
        dump["instance-type"] = Value::String(type_name.to_string());
        one_of.push(dump);
    }

    let mut type_schema = dump_schema(schema.type_schema());
    if schema.type_property_entry.1 {
        type_schema["optional"] = true.into();
    }

    json!({
        "type": "object",
        "description": schema.description,
        "type-property": schema.type_property(),
        "type-property-schema": type_schema,
        "oneOf": one_of,
    })
}

fn dump_api_permission(permission: &Permission, privileges: &[(&str, u64)]) -> Value {
    match permission {
        Permission::Superuser => json!({ "user": "root@pam" }),
        Permission::User(user) => json!({ "user": user }),
        Permission::Anybody => json!({ "user": "all" }),
        Permission::World => json!({ "user": "world" }),
        Permission::UserParam(param) => json!({ "userParam": param }),
        Permission::Group(group) => json!({ "group": group }),
        Permission::WithParam(param, sub_permission) => {
            json!({
                "withParam": {
                    "name": param,
                    "permissions": dump_api_permission(sub_permission, privileges),
                },
            })
        }
        Permission::Privilege(name, value, partial) => {
            let mut privs = Vec::new();
            for (name, v) in privileges {
                if (value & v) != 0 {
                    privs.push(name.to_string());
                }
            }

            json!({
                "check": {
                    "path": name,
                    "privs": privs,
                    "partial": partial,
                }
            })
        }
        Permission::And(list) => {
            let list: Vec<Value> = list
                .iter()
                .map(|p| dump_api_permission(p, privileges))
                .collect();
            json!({ "and": list })
        }
        Permission::Or(list) => {
            let list: Vec<Value> = list
                .iter()
                .map(|p| dump_api_permission(p, privileges))
                .collect();
            json!({ "or": list })
        }
    }
}

fn dump_api_method_schema(
    method: &str,
    api_method: &ApiMethod,
    privileges: &[(&str, u64)],
) -> Value {
    let mut data = json!({
        "description": api_method.parameters.description(),
    });

    data["parameters"] = dump_parameter_schema(&api_method.parameters);

    let mut returns = dump_schema(api_method.returns.schema);
    if api_method.returns.optional {
        returns["optional"] = 1.into();
    }
    data["returns"] = returns;
    data["unstable"] = api_method.unstable.into();

    match api_method.access {
        ApiAccess {
            description: None,
            permission: Permission::Superuser,
        } => {
            // no need to output default
        }
        ApiAccess {
            description,
            permission,
        } => {
            let mut permissions = dump_api_permission(permission, privileges);
            if let Some(description) = description {
                permissions["description"] = description.into();
            }
            data["permissions"] = permissions;
        }
    }

    let mut method = method;

    if let ApiHandler::AsyncHttp(_) = api_method.handler {
        method = if method == "POST" { "UPLOAD" } else { method };
        method = if method == "GET" { "DOWNLOAD" } else { method };
    }

    data["method"] = method.into();

    data
}

fn dump_api_schema(router: &Router, path: &str, privileges: &[(&str, u64)]) -> Value {
    let mut data = json!({});

    let mut info = json!({});
    if let Some(api_method) = router.get {
        info["GET"] = dump_api_method_schema("GET", api_method, privileges);
    }
    if let Some(api_method) = router.post {
        info["POST"] = dump_api_method_schema("POST", api_method, privileges);
    }
    if let Some(api_method) = router.put {
        info["PUT"] = dump_api_method_schema("PUT", api_method, privileges);
    }
    if let Some(api_method) = router.delete {
        info["DELETE"] = dump_api_method_schema("DELETE", api_method, privileges);
    }

    data["info"] = info;

    match &router.subroute {
        None => {
            data["leaf"] = 1.into();
        }
        Some(SubRoute::MatchAll { router, param_name }) => {
            let sub_path = if path == "." {
                format!("/{{{param_name}}}")
            } else {
                format!("{path}/{{{param_name}}}")
            };
            let mut child = dump_api_schema(router, &sub_path, privileges);
            child["path"] = sub_path.into();
            child["text"] = format!("{{{param_name}}}").into();

            let children = vec![child];
            data["children"] = children.into();
            data["leaf"] = 0.into();
        }
        Some(SubRoute::Map(dirmap)) => {
            let mut children = Vec::new();

            for (key, sub_router) in dirmap.iter() {
                let sub_path = if path == "." {
                    format!("/{key}")
                } else {
                    format!("{path}/{key}")
                };
                let mut child = dump_api_schema(sub_router, &sub_path, privileges);
                child["path"] = sub_path.into();
                child["text"] = key.to_string().into();
                children.push(child);
            }

            data["children"] = children.into();
            data["leaf"] = 0.into();
        }
    }

    data
}
