use indexmap::IndexMap;

use crate::{
    ast::{Directive, FromInputValue, InputValue, Selection},
    executor::{ExecutionResult, Executor, Registry, Variables},
    parser::Spanning,
    schema::meta::{Argument, MetaType},
    value::{DefaultScalarValue, Object, ScalarValue, Value},
    GraphQLEnum,
};

/// GraphQL type kind
///
/// The GraphQL specification defines a number of type kinds - the meta type\
/// of a type.
#[derive(Clone, Eq, PartialEq, Debug, GraphQLEnum)]
#[graphql(name = "__TypeKind", internal)]
pub enum TypeKind {
    /// ## Scalar types
    ///
    /// Scalar types appear as the leaf nodes of GraphQL queries. Strings,\
    /// numbers, and booleans are the built in types, and while it's possible\
    /// to define your own, it's relatively uncommon.
    Scalar,

    /// ## Object types
    ///
    /// The most common type to be implemented by users. Objects have fields\
    /// and can implement interfaces.
    Object,

    /// ## Interface types
    ///
    /// Interface types are used to represent overlapping fields between\
    /// multiple types, and can be queried for their concrete type.
    Interface,

    /// ## Union types
    ///
    /// Unions are similar to interfaces but can not contain any fields on\
    /// their own.
    Union,

    /// ## Enum types
    ///
    /// Like scalars, enum types appear as the leaf nodes of GraphQL queries.
    Enum,

    /// ## Input objects
    ///
    /// Represents complex values provided in queries _into_ the system.
    #[graphql(name = "INPUT_OBJECT")]
    InputObject,

    /// ## List types
    ///
    /// Represent lists of other types. This library provides implementations\
    /// for vectors and slices, but other Rust types can be extended to serve\
    /// as GraphQL lists.
    List,

    /// ## Non-null types
    ///
    /// In GraphQL, nullable types are the default. By putting a `!` after a\
    /// type, it becomes non-nullable.
    #[graphql(name = "NON_NULL")]
    NonNull,
}

/// Field argument container
#[derive(Debug)]
pub struct Arguments<'a, S = DefaultScalarValue> {
    args: Option<IndexMap<&'a str, InputValue<S>>>,
}

impl<'a, S> Arguments<'a, S>
where
    S: ScalarValue,
{
    #[doc(hidden)]
    pub fn new(
        mut args: Option<IndexMap<&'a str, InputValue<S>>>,
        meta_args: &'a Option<Vec<Argument<S>>>,
    ) -> Self {
        if meta_args.is_some() && args.is_none() {
            args = Some(IndexMap::new());
        }

        if let (&mut Some(ref mut args), &Some(ref meta_args)) = (&mut args, meta_args) {
            for arg in meta_args {
                if !args.contains_key(arg.name.as_str()) || args[arg.name.as_str()].is_null() {
                    if let Some(ref default_value) = arg.default_value {
                        args.insert(arg.name.as_str(), default_value.clone());
                    }
                }
            }
        }

        Arguments { args }
    }

    /// Get and convert an argument into the desired type.
    ///
    /// If the argument is found, or a default argument has been provided,
    /// the `InputValue` will be converted into the type `T`.
    ///
    /// Returns `Some` if the argument is present _and_ type conversion
    /// succeeeds.
    pub fn get<T>(&self, key: &str) -> Option<T>
    where
        T: FromInputValue<S>,
    {
        self.args
            .as_ref()
            .and_then(|args| args.get(key))
            .and_then(InputValue::convert)
    }

    /// Get an interator over the arg values
    pub fn iter(&self) -> Option<impl Iterator<Item = (&&str, &InputValue<S>)>> {
        self.args.as_ref().map(|args| args.iter())
    }
}

/// Primary trait used to resolve GraphQL values.
///
/// All the convenience macros ultimately expand into an implementation of this trait for the given
/// type. The macros remove duplicated definitions of fields and arguments, and add type checks on
/// all resolving functions automatically. This can all be done manually too.
///
/// [`GraphQLValue`] provides _some_ convenience methods for you, in the form of optional trait
/// methods. The `type_name` method is mandatory, but other than that, it depends on what type
/// you're exposing:
/// - [Scalars][4], [enums][5], [lists][6] and [non-null wrappers][7] only require `resolve`.
/// - [Interfaces][1] and [objects][3] require `resolve_field` _or_ `resolve` if you want to
///   implement a custom resolution logic (probably not).
/// - [Interfaces][1] and [unions][2] require `resolve_into_type` and `concrete_type_name`.
/// - [Input objects][8] do not require anything.
///
/// # Object safety
///
/// This trait is [object safe][11], therefore may be turned into a [trait object][12] and used for
/// resolving GraphQL values even when a concrete Rust type is erased.
///
/// # Example
///
/// This trait is intended to be used in a conjunction with a [`GraphQLType`] trait. See the example
/// in the documentation of a [`GraphQLType`] trait.
///
/// [1]: https://spec.graphql.org/June2018/#sec-Interfaces
/// [2]: https://spec.graphql.org/June2018/#sec-Unions
/// [3]: https://spec.graphql.org/June2018/#sec-Objects
/// [4]: https://spec.graphql.org/June2018/#sec-Scalars
/// [5]: https://spec.graphql.org/June2018/#sec-Enums
/// [6]: https://spec.graphql.org/June2018/#sec-Type-System.List
/// [7]: https://spec.graphql.org/June2018/#sec-Type-System.Non-Null
/// [8]: https://spec.graphql.org/June2018/#sec-Input-Objects
/// [11]: https://doc.rust-lang.org/reference/items/traits.html#object-safety
/// [12]: https://doc.rust-lang.org/reference/types/trait-object.html
pub trait GraphQLValue<S = DefaultScalarValue>
where
    S: ScalarValue,
{
    /// Context type for this [`GraphQLValue`].
    ///
    /// It's threaded through a query execution to all affected nodes, and can be used to hold
    /// common data, e.g. database connections or request session information.
    type Context;

    /// Type that may carry additional schema information for this [`GraphQLValue`].
    ///
    /// It can be used to implement a schema that is partly dynamic, meaning that it can use
    /// information that is not known at compile time, for instance by reading it from a
    /// configuration file at startup.
    type TypeInfo;

    /// Returns name of the [`GraphQLType`] exposed by this [`GraphQLValue`].
    ///
    /// This function will be called multiple times during a query execution. It must _not_ perform
    /// any calculation and _always_ return the same value.
    ///
    /// Usually, it should just call a [`GraphQLType::name`] inside.
    fn type_name<'i>(&self, info: &'i Self::TypeInfo) -> Option<&'i str>;

    /// Resolves the value of a single field on this [`GraphQLValue`].
    ///
    /// The `arguments` object contains all the specified arguments, with default values being
    /// substituted for the ones not provided by the query.
    ///
    /// The `executor` can be used to drive selections into sub-[objects][3].
    ///
    /// # Panics
    ///
    /// The default implementation panics.
    ///
    /// [3]: https://spec.graphql.org/June2018/#sec-Objects
    fn resolve_field(
        &self,
        _info: &Self::TypeInfo,
        _field_name: &str,
        _arguments: &Arguments<S>,
        _executor: &Executor<Self::Context, S>,
    ) -> ExecutionResult<S> {
        panic!("GraphQLValue::resolve_field() must be implemented by objects and interfaces");
    }

    /// Resolves this [`GraphQLValue`] (being an [interface][1] or an [union][2]) into a concrete
    /// downstream [object][3] type.
    ///
    /// Tries to resolve this [`GraphQLValue`] into the provided `type_name`. If the type matches,
    /// then passes the instance along to [`Executor::resolve`].
    ///
    /// # Panics
    ///
    /// The default implementation panics.
    ///
    /// [1]: https://spec.graphql.org/June2018/#sec-Interfaces
    /// [2]: https://spec.graphql.org/June2018/#sec-Unions
    /// [3]: https://spec.graphql.org/June2018/#sec-Objects
    fn resolve_into_type(
        &self,
        info: &Self::TypeInfo,
        type_name: &str,
        selection_set: Option<&[Selection<S>]>,
        executor: &Executor<Self::Context, S>,
    ) -> ExecutionResult<S> {
        if self.type_name(info).unwrap() == type_name {
            self.resolve(info, selection_set, executor)
        } else {
            panic!(
                "GraphQLValue::resolve_into_type() must be implemented by unions and interfaces"
            );
        }
    }

    /// Returns the concrete [`GraphQLType`] name for this [`GraphQLValue`] being an [interface][1],
    /// an [union][2] or an [object][3].
    ///
    /// # Panics
    ///
    /// The default implementation panics.
    ///
    /// [1]: https://spec.graphql.org/June2018/#sec-Interfaces
    /// [2]: https://spec.graphql.org/June2018/#sec-Unions
    /// [3]: https://spec.graphql.org/June2018/#sec-Objects
    #[allow(unused_variables)]
    fn concrete_type_name(&self, context: &Self::Context, info: &Self::TypeInfo) -> String {
        panic!(
            "GraphQLValue::concrete_type_name() must be implemented by unions, interfaces \
             and objects",
        );
    }

    /// Resolves the provided `selection_set` against this [`GraphQLValue`].
    ///
    /// For non-[object][3] types, the `selection_set` will be [`None`] and the value should simply
    /// be returned.
    ///
    /// For [objects][3], all fields in the `selection_set` should be resolved. The default
    /// implementation uses [`GraphQLValue::resolve_field`] to resolve all fields, including those
    /// through a fragment expansion.
    ///
    /// Since the [GraphQL spec specifies][0] that errors during field processing should result in
    /// a null-value, this might return `Ok(Null)` in case of a failure. Errors are recorded
    /// internally.
    ///
    /// # Panics
    ///
    /// The default implementation panics, if `selection_set` is [`None`].
    ///
    /// [0]: https://spec.graphql.org/June2018/#sec-Errors-and-Non-Nullability
    /// [3]: https://spec.graphql.org/June2018/#sec-Objects
    fn resolve(
        &self,
        info: &Self::TypeInfo,
        selection_set: Option<&[Selection<S>]>,
        executor: &Executor<Self::Context, S>,
    ) -> ExecutionResult<S> {
        if let Some(sel) = selection_set {
            let mut res = Object::with_capacity(sel.len());
            Ok(
                if resolve_selection_set_into(self, info, sel, executor, &mut res) {
                    Value::Object(res)
                } else {
                    Value::null()
                },
            )
        } else {
            panic!("GraphQLValue::resolve() must be implemented by non-object output types");
        }
    }
}

crate::sa::assert_obj_safe!(GraphQLValue<Context = (), TypeInfo = ()>);

/// Helper alias for naming [trait objects][1] of [`GraphQLValue`].
///
/// [1]: https://doc.rust-lang.org/reference/types/trait-object.html
pub type DynGraphQLValue<S, C, TI> =
    dyn GraphQLValue<S, Context = C, TypeInfo = TI> + Send + Sync + 'static;

/// Primary trait used to expose Rust types in a GraphQL schema.
///
/// All of the convenience macros ultimately expand into an implementation of
/// this trait for the given type. This can all be done manually.
///
/// # Example
///
/// Manually deriving an [object][3] is straightforward, but tedious. This is the equivalent of the
/// `User` object as shown in the example in the documentation root:
/// ```
/// # use std::collections::HashMap;
/// use juniper::{
///     meta::MetaType, Arguments, Context, DefaultScalarValue, Executor, ExecutionResult,
///     FieldResult, GraphQLType, GraphQLValue, Registry,
/// };
///
/// #[derive(Debug)]
/// struct Database { users: HashMap<String, User> }
/// impl Context for Database {}
///
/// #[derive(Debug)]
/// struct User { id: String, name: String, friend_ids: Vec<String> }
///
/// impl GraphQLType<DefaultScalarValue> for User {
///    fn name(_: &()) -> Option<&'static str> {
///        Some("User")
///    }
///
///    fn meta<'r>(_: &(), registry: &mut Registry<'r>) -> MetaType<'r>
///    where DefaultScalarValue: 'r,
///    {
///        // First, we need to define all fields and their types on this type.
///        //
///        // If we need arguments, want to implement interfaces, or want to add documentation
///        // strings, we can do it here.
///        let fields = &[
///            registry.field::<&String>("id", &()),
///            registry.field::<&String>("name", &()),
///            registry.field::<Vec<&User>>("friends", &()),
///        ];
///        registry.build_object_type::<User>(&(), fields).into_meta()
///    }
/// }
///
/// impl GraphQLValue<DefaultScalarValue> for User {
///     type Context = Database;
///     type TypeInfo = ();
///
///     fn type_name(&self, _: &()) -> Option<&'static str> {
///         <User as GraphQLType>::name(&())
///     }
///
///     fn resolve_field(
///         &self,
///         info: &(),
///         field_name: &str,
///         args: &Arguments,
///         executor: &Executor<Database>
///     ) -> ExecutionResult
///     {
///         // Next, we need to match the queried field name. All arms of this match statement
///         // return `ExecutionResult`, which makes it hard to statically verify that the type you
///         // pass on to `executor.resolve*` actually matches the one that you defined in `meta()`
///         // above.
///         let database = executor.context();
///         match field_name {
///             // Because scalars are defined with another `Context` associated type, you must use
///             // `resolve_with_ctx` here to make the `executor` perform automatic type conversion
///             // of its argument.
///             "id" => executor.resolve_with_ctx(info, &self.id),
///             "name" => executor.resolve_with_ctx(info, &self.name),
///
///             // You pass a vector of `User` objects to `executor.resolve`, and it will determine
///             // which fields of the sub-objects to actually resolve based on the query.
///             // The `executor` instance keeps track of its current position in the query.
///             "friends" => executor.resolve(info,
///                 &self.friend_ids.iter()
///                     .filter_map(|id| database.users.get(id))
///                     .collect::<Vec<_>>()
///             ),
///
///             // We can only reach this panic in two cases: either a mismatch between the defined
///             // schema in `meta()` above, or a validation failed because of a this library bug.
///             //
///             // In either of those two cases, the only reasonable way out is to panic the thread.
///             _ => panic!("Field {} not found on type User", field_name),
///         }
///     }
/// }
/// ```
pub trait GraphQLType<S = DefaultScalarValue>: GraphQLValue<S>
where
    S: ScalarValue,
{
    /// Returns name of this [`GraphQLType`] to expose.
    ///
    /// This function will be called multiple times during schema construction. It must _not_
    /// perform any calculation and _always_ return the same value.
    fn name(info: &Self::TypeInfo) -> Option<&str>;

    /// Returns [`MetaType`] representing this [`GraphQLType`].
    fn meta<'r>(info: &Self::TypeInfo, registry: &mut Registry<'r, S>) -> MetaType<'r, S>
    where
        S: 'r;
}

/// Resolver logic for queries'/mutations' selection set.
/// Calls appropriate resolver method for each field or fragment found
/// and then merges returned values into `result` or pushes errors to
/// field's/fragment's sub executor.
///
/// Returns false if any errors occurred and true otherwise.
pub(crate) fn resolve_selection_set_into<T, S>(
    instance: &T,
    info: &T::TypeInfo,
    selection_set: &[Selection<S>],
    executor: &Executor<T::Context, S>,
    result: &mut Object<S>,
) -> bool
where
    T: GraphQLValue<S> + ?Sized,
    S: ScalarValue,
{
    let meta_type = executor
        .schema()
        .concrete_type_by_name(
            instance
                .type_name(info)
                .expect("Resolving named type's selection set")
                .as_ref(),
        )
        .expect("Type not found in schema");

    for selection in selection_set {
        match *selection {
            Selection::Field(Spanning {
                item: ref f,
                start: ref start_pos,
                ..
            }) => {
                if is_excluded(&f.directives, executor.variables()) {
                    continue;
                }

                let response_name = f.alias.as_ref().unwrap_or(&f.name).item;

                if f.name.item == "__typename" {
                    result.add_field(
                        response_name,
                        Value::scalar(instance.concrete_type_name(executor.context(), info)),
                    );
                    continue;
                }

                let meta_field = meta_type.field_by_name(f.name.item).unwrap_or_else(|| {
                    panic!(
                        "Field {} not found on type {:?}",
                        f.name.item,
                        meta_type.name()
                    )
                });

                let exec_vars = executor.variables();

                let sub_exec = executor.field_sub_executor(
                    response_name,
                    f.name.item,
                    *start_pos,
                    f.selection_set.as_ref().map(|v| &v[..]),
                );

                let field_result = instance.resolve_field(
                    info,
                    f.name.item,
                    &Arguments::new(
                        f.arguments.as_ref().map(|m| {
                            m.item
                                .iter()
                                .map(|&(ref k, ref v)| {
                                    (k.item, v.item.clone().into_const(exec_vars))
                                })
                                .collect()
                        }),
                        &meta_field.arguments,
                    ),
                    &sub_exec,
                );

                match field_result {
                    Ok(Value::Null) if meta_field.field_type.is_non_null() => return false,
                    Ok(v) => merge_key_into(result, response_name, v),
                    Err(e) => {
                        sub_exec.push_error_at(e, *start_pos);

                        if meta_field.field_type.is_non_null() {
                            return false;
                        }

                        result.add_field(response_name, Value::null());
                    }
                }
            }
            Selection::FragmentSpread(Spanning {
                item: ref spread,
                start: ref start_pos,
                ..
            }) => {
                if is_excluded(&spread.directives, executor.variables()) {
                    continue;
                }

                let fragment = &executor
                    .fragment_by_name(spread.name.item)
                    .expect("Fragment could not be found");

                let sub_exec = executor.type_sub_executor(
                    Some(fragment.type_condition.item),
                    Some(&fragment.selection_set[..]),
                );

                let concrete_type_name = instance.concrete_type_name(sub_exec.context(), info);
                if fragment.type_condition.item == concrete_type_name {
                    let sub_result = instance.resolve_into_type(
                        info,
                        fragment.type_condition.item,
                        Some(&fragment.selection_set[..]),
                        &sub_exec,
                    );

                    if let Ok(Value::Object(object)) = sub_result {
                        for (k, v) in object {
                            merge_key_into(result, &k, v);
                        }
                    } else if let Err(e) = sub_result {
                        sub_exec.push_error_at(e, *start_pos);
                    }
                }
            }
            Selection::InlineFragment(Spanning {
                item: ref fragment,
                start: ref start_pos,
                ..
            }) => {
                if is_excluded(&fragment.directives, executor.variables()) {
                    continue;
                }

                let sub_exec = executor.type_sub_executor(
                    fragment.type_condition.as_ref().map(|c| c.item),
                    Some(&fragment.selection_set[..]),
                );

                if let Some(ref type_condition) = fragment.type_condition {
                    // Check whether the type matches the type condition.
                    let concrete_type_name = instance.concrete_type_name(sub_exec.context(), info);
                    if type_condition.item == concrete_type_name {
                        let sub_result = instance.resolve_into_type(
                            info,
                            type_condition.item,
                            Some(&fragment.selection_set[..]),
                            &sub_exec,
                        );

                        if let Ok(Value::Object(object)) = sub_result {
                            for (k, v) in object {
                                merge_key_into(result, &k, v);
                            }
                        } else if let Err(e) = sub_result {
                            sub_exec.push_error_at(e, *start_pos);
                        }
                    }
                } else if !resolve_selection_set_into(
                    instance,
                    info,
                    &fragment.selection_set[..],
                    &sub_exec,
                    result,
                ) {
                    return false;
                }
            }
        }
    }

    true
}

pub(super) fn is_excluded<S>(
    directives: &Option<Vec<Spanning<Directive<S>>>>,
    vars: &Variables<S>,
) -> bool
where
    S: ScalarValue,
{
    if let Some(ref directives) = *directives {
        for &Spanning {
            item: ref directive,
            ..
        } in directives
        {
            let condition: bool = directive
                .arguments
                .iter()
                .flat_map(|m| m.item.get("if"))
                .flat_map(|v| v.item.clone().into_const(vars).convert())
                .next()
                .unwrap();

            if (directive.name.item == "skip" && condition)
                || (directive.name.item == "include" && !condition)
            {
                return true;
            }
        }
    }
    false
}

/// Merges `response_name`/`value` pair into `result`
pub(crate) fn merge_key_into<S>(result: &mut Object<S>, response_name: &str, value: Value<S>) {
    result.add_field(response_name, value);
}
