"""Admission rules for the three pure-function fixtures, not a Python sandbox."""

import ast
import builtins

ALLOWED_BUILTINS = {
    "abs",
    "all",
    "any",
    "bool",
    "dict",
    "enumerate",
    "float",
    "int",
    "isinstance",
    "len",
    "list",
    "max",
    "min",
    "range",
    "reversed",
    "set",
    "sorted",
    "str",
    "sum",
    "super",
    "tuple",
    "zip",
    "Exception",
    "IndexError",
    "KeyError",
    "NotImplementedError",
    "RuntimeError",
    "TypeError",
    "ValueError",
}
ALLOWED_ATTRIBUTES = {
    "add",
    "append",
    "casefold",
    "clear",
    "copy",
    "count",
    "extend",
    "get",
    "index",
    "insert",
    "items",
    "join",
    "keys",
    "line_number",
    "lower",
    "pop",
    "remove",
    "reverse",
    "sort",
    "split",
    "splitlines",
    "strip",
    "update",
    "upper",
    "values",
    "loads",
    "JSONDecodeError",
}
ALLOWED_NODES = {
    "Module",
    "FunctionDef",
    "arguments",
    "arg",
    "Return",
    "Assign",
    "AnnAssign",
    "AugAssign",
    "Expr",
    "Pass",
    "If",
    "IfExp",
    "For",
    "While",
    "Break",
    "Continue",
    "Try",
    "ExceptHandler",
    "Raise",
    "Assert",
    "Import",
    "alias",
    "ClassDef",
    "Name",
    "Load",
    "Store",
    "Constant",
    "List",
    "Tuple",
    "Set",
    "Dict",
    "Subscript",
    "Slice",
    "Attribute",
    "Call",
    "keyword",
    "ListComp",
    "SetComp",
    "DictComp",
    "GeneratorExp",
    "comprehension",
    "Compare",
    "BoolOp",
    "BinOp",
    "UnaryOp",
    "JoinedStr",
    "FormattedValue",
    "And",
    "Or",
    "Not",
    "Add",
    "Sub",
    "Mult",
    "Div",
    "FloorDiv",
    "Mod",
    "Pow",
    "USub",
    "UAdd",
    "Eq",
    "NotEq",
    "Lt",
    "LtE",
    "Gt",
    "GtE",
    "Is",
    "IsNot",
    "In",
    "NotIn",
}


def admitted_source(source):
    if len(source.encode()) > 64_000:
        return False
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError, RecursionError):
        return False
    nodes = list(ast.walk(tree))
    if len(nodes) > 8_000:
        return False
    functions = {node.name for node in nodes if isinstance(node, ast.FunctionDef)}
    callable_names = ALLOWED_BUILTINS | functions | {"RecordError"}
    forbidden_names = set(vars(builtins)) - ALLOWED_BUILTINS
    for node in nodes:
        if type(node).__name__ not in ALLOWED_NODES:
            return False
        if isinstance(node, ast.Name):
            if node.id.startswith("__") or node.id in forbidden_names:
                return False
        if isinstance(node, ast.Import):
            if (
                len(node.names) != 1
                or node.names[0].name != "json"
                or node.names[0].asname
            ):
                return False
        if isinstance(node, ast.FunctionDef):
            if node.decorator_list or (
                node.name.startswith("__") and node.name != "__init__"
            ):
                return False
        if isinstance(node, ast.ClassDef):
            if (
                node.name != "RecordError"
                or node.decorator_list
                or node.keywords
                or len(node.bases) != 1
                or not isinstance(node.bases[0], ast.Name)
                or node.bases[0].id != "ValueError"
            ):
                return False
            for member in node.body:
                if isinstance(member, ast.FunctionDef) and member.name == "__init__":
                    continue
                if isinstance(member, ast.Pass):
                    continue
                if (
                    isinstance(member, ast.Expr)
                    and isinstance(member.value, ast.Constant)
                    and isinstance(member.value.value, str)
                ):
                    continue
                return False
        if isinstance(node, ast.Attribute):
            initializer = (
                node.attr == "__init__"
                and isinstance(node.value, ast.Call)
                and isinstance(node.value.func, ast.Name)
                and node.value.func.id == "super"
                and not node.value.args
                and not node.value.keywords
            )
            if node.attr not in ALLOWED_ATTRIBUTES and not initializer:
                return False
            if isinstance(node.ctx, ast.Store) and not (
                node.attr == "line_number"
                and isinstance(node.value, ast.Name)
                and node.value.id == "self"
            ):
                return False
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Name):
                if node.func.id not in callable_names:
                    return False
            elif not isinstance(node.func, ast.Attribute):
                return False
    return True
