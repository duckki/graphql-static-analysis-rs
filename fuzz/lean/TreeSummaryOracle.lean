import GraphQL.Theories.TreeSummary.ExactCases
import GraphQL.Theories.TreeSummary.StaticCost
import GraphQL.Theories.TreeSummary.Syntactic

/-! Persistent native differential oracle for the Rust TreeSummary implementation. -/

namespace GraphQLStaticAnalysisFuzz

open GraphQL
open GraphQL.TreeSummary
open GraphQL.ConditionTree

structure Profile where
  bytes : List Nat
  family : Nat
  variableCase : Nat
  mode : Nat
  listSize : Nat
  observation : Nat
  defaultCase : Nat
  structural : Bool

def byteAt (bytes : List Nat) (index : Nat) : Nat :=
  (bytes[index]?).getD 0

def profileFromBytes (bytes : List Nat) : Profile :=
  {
    bytes
    family := byteAt bytes 0 % 12
    variableCase := byteAt bytes 1 % 10
    mode := byteAt bytes 2 % 2
    listSize := byteAt bytes 3 % 5
    observation := byteAt bytes 4 % 4
    defaultCase := byteAt bytes 5 % 3
    structural := byteAt bytes 6 % 2 == 1
  }

def leafField (name : Name) : FieldDefinition :=
  { name, outputType := .named "String" }

def commonFields : List FieldDefinition :=
  [
    leafField "name",
    leafField "id",
    { name := "friend", outputType := .named "Animal" },
    { name := "friends", outputType := .list (.named "Animal") }
  ]

def schema : Schema :=
  {
    queryType := "Query"
    types :=
      [
        .object
          {
            name := "Query"
            fields := [{ name := "animals", outputType := .list (.named "Animal") }]
          },
        .interface { name := "Animal", fields := commonFields },
        .interface { name := "I", fields := commonFields },
        .interface { name := "J", fields := commonFields },
        .interface { name := "K", fields := commonFields },
        .object
          {
            name := "Dog"
            fields := commonFields
            interfaces := ["Animal", "I", "J", "K"]
          },
        .object
          {
            name := "Cat"
            fields := commonFields
            interfaces := ["Animal", "I", "K"]
          },
        .object
          {
            name := "Fox"
            fields := commonFields
            interfaces := ["Animal", "J"]
          }
      ]
  }

def field (responseName fieldName : Name)
    (directives : List DirectiveApplication := [])
    (children : List Selection := []) : Selection :=
  .field responseName fieldName [] directives children

def includeValue (value : InputValue) : DirectiveApplication := .include value
def skipValue (value : InputValue) : DirectiveApplication := .skip value
def includeVariable (name : Name) : DirectiveApplication := includeValue (.variable name)
def skipVariable (name : Name) : DirectiveApplication := skipValue (.variable name)

def legacySelectionSet : Nat -> List Selection
  | 0 => [field "name" "name"]
  | 1 => [field "name" "name" [skipVariable "x"]]
  | 2 => [field "name" "name" [includeVariable "x"]]
  | 3 =>
      [
        field "included" "name" [includeVariable "x"],
        field "skipped" "id" [skipVariable "x"]
      ]
  | 4 =>
      [field "nameLabel" "name", field "nameLabel" "name" [includeVariable "x"]]
  | 5 =>
      [
        field "a" "name" [includeVariable "x"],
        field "b" "id" [includeVariable "y"]
      ]
  | 6 =>
      [
        .inlineFragment none [includeVariable "x"]
          [.inlineFragment none [skipVariable "y"] [field "name" "name"]]
      ]
  | 7 =>
      [
        .inlineFragment (some "I") [] [field "i" "name"],
        .inlineFragment (some "J") [] [field "j" "id"]
      ]
  | 8 =>
      [
        field "nameLabel" "name",
        .inlineFragment (some "Dog") [] [field "nameLabel" "name"]
      ]
  | 9 =>
      [
        field "nameLabel" "name",
        .inlineFragment none [includeVariable "x"]
          [.inlineFragment none [includeVariable "y"] [field "nameLabel" "name"]]
      ]
  | 10 =>
      [
        .inlineFragment (some "I") [includeVariable "x"] [field "i" "name"],
        .inlineFragment (some "J") [skipVariable "y"] [field "j" "id"]
      ]
  | _ =>
      [
        .inlineFragment (some "Dog") [] [field "dog" "name"],
        .inlineFragment (some "Cat") [] [field "cat" "id"]
      ]

structure Cursor where
  remaining : List Nat

def Cursor.next : Cursor -> Nat × Cursor
  | { remaining := [] } => (0, { remaining := [] })
  | { remaining := value :: rest } => (value, { remaining := rest })

def directiveApplications (token : Nat) : List DirectiveApplication :=
  match token % 16 with
  | 0 => []
  | 1 => [includeVariable "x"]
  | 2 => [skipVariable "x"]
  | 3 => [includeVariable "y"]
  | 4 => [skipVariable "y"]
  | 5 => [includeValue (.boolean true)]
  | 6 => [includeValue (.boolean false)]
  | 7 => [skipValue (.boolean true)]
  | 8 => [skipValue (.boolean false)]
  | 9 => [includeVariable "x", skipVariable "y"]
  | 10 => [includeVariable "y", skipVariable "x"]
  | 11 => [includeVariable "x", skipVariable "x"]
  | 12 => [includeVariable "y", skipVariable "y"]
  | 13 => [includeValue (.boolean true), skipVariable "x"]
  | 14 => [includeVariable "x", skipValue (.boolean false)]
  | _ => [includeValue (.boolean false), skipValue (.boolean true)]

def scalarResponseName (fieldName : Name) (token : Nat) : Name :=
  match fieldName, token % 3 with
  | "name", 0 => "name"
  | "name", 1 => "nameLabel"
  | "name", _ => "nameValue"
  | "id", 0 => "id"
  | "id", 1 => "identifier"
  | _, _ => "idValue"

def compositeResponseName (fieldName : Name) (token : Nat) : Name :=
  match fieldName, token % 2 with
  | "friend", 0 => "friend"
  | "friend", _ => "node"
  | "friends", 0 => "friends"
  | _, _ => "nodes"

def typeCondition (token : Nat) : Option Name :=
  match token % 5 with
  | 0 => none
  | 1 => some "Animal"
  | 2 => some "I"
  | 3 => some "J"
  | _ => some "K"

instance : Inhabited Selection :=
  ⟨field "name" "name"⟩

mutual
  partial def structuralSelectionSet (depth : Nat) (cursor : Cursor)
      : List Selection × Cursor :=
    let (countToken, cursor) := cursor.next
    structuralSelections (1 + countToken % 3) depth cursor

  partial def structuralSelections (count depth : Nat) (cursor : Cursor)
      : List Selection × Cursor :=
    match count with
    | 0 => ([], cursor)
    | count + 1 =>
        let (selection, cursor) := structuralSelection depth cursor
        let (rest, cursor) := structuralSelections count depth cursor
        (selection :: rest, cursor)

  partial def structuralSelection (depth : Nat) (cursor : Cursor) : Selection × Cursor :=
    let (tag, cursor) := cursor.next
    let kind := if depth == 0 then tag % 2 else tag % 4
    match kind with
    | 0 | 1 =>
        let fieldName := if kind == 0 then "name" else "id"
        let (aliasToken, cursor) := cursor.next
        let (directiveToken, cursor) := cursor.next
        (
          field (scalarResponseName fieldName aliasToken) fieldName
            (directiveApplications directiveToken),
          cursor
        )
    | 2 =>
        let (fieldToken, cursor) := cursor.next
        let fieldName := if fieldToken % 2 == 0 then "friend" else "friends"
        let (aliasToken, cursor) := cursor.next
        let (directiveToken, cursor) := cursor.next
        let (children, cursor) := structuralSelectionSet (depth - 1) cursor
        (
          field (compositeResponseName fieldName aliasToken) fieldName
            (directiveApplications directiveToken) children,
          cursor
        )
    | _ =>
        let (typeToken, cursor) := cursor.next
        let (directiveToken, cursor) := cursor.next
        let (children, cursor) := structuralSelectionSet (depth - 1) cursor
        (
          .inlineFragment (typeCondition typeToken)
            (directiveApplications directiveToken) children,
          cursor
        )
end

def profileSelectionSet (profile : Profile) : List Selection :=
  if profile.structural then
    let cursor : Cursor := { remaining := profile.bytes.drop 7 }
    let (depthToken, cursor) := cursor.next
    (structuralSelectionSet (1 + depthToken % 3) cursor).1
  else
    legacySelectionSet profile.family

def inputVariable? : InputValue -> Option Name
  | .variable name => some name
  | _ => none

def directiveVariable? : DirectiveApplication -> Option Name
  | .include value => inputVariable? value
  | .skip value => inputVariable? value

mutual
  partial def selectionVariables : Selection -> List Name
    | .field _responseName _fieldName _arguments directives children =>
        directives.filterMap directiveVariable? ++ selectionSetVariables children
    | .inlineFragment _typeCondition directives children =>
        directives.filterMap directiveVariable? ++ selectionSetVariables children

  partial def selectionSetVariables : List Selection -> List Name
    | [] => []
    | selection :: rest => selectionVariables selection ++ selectionSetVariables rest
end

def variableDefault : Nat -> Option ConstInputValue
  | 1 => some (.boolean false)
  | 2 => some (.boolean true)
  | _ => none

def operation (profile : Profile) : Operation :=
  let selections := profileSelectionSet profile
  let defaultValue := variableDefault profile.defaultCase
  let variableDefinitions :=
    (selectionSetVariables selections).eraseDups.map fun name =>
      ({ name, typeRef := .nonNull (.named "Boolean"), defaultValue } : VariableDefinition)
  {
    name := some "Case"
    variableDefinitions
    selectionSet := [.field "animals" "animals" [] [] selections]
  }

def variableValues : Nat -> Execution.VariableValues
  | 0 => []
  | 1 => []
  | 2 => [("x", .boolean false), ("y", .boolean false)]
  | 3 => [("x", .boolean true), ("y", .boolean true)]
  | 4 => [("x", .boolean false), ("y", .boolean true)]
  | 5 => [("x", .boolean true), ("y", .boolean false)]
  | 6 => [("x", .null), ("y", .null)]
  | 7 => [("x", .boolean true)]
  | 8 => [("y", .boolean true)]
  | _ => [("x", .string "non-boolean"), ("y", .int 1)]

def listMultiplier (listSize : Nat) : TypeRef -> Nat
  | .named _typeName => 1
  | .list inner => listSize * listMultiplier listSize inner
  | .nonNull inner => listMultiplier listSize inner

def fieldListMultiplier (listSize : Nat) (group : CollectedFieldGroup) : Nat :=
  (group.fieldOutputTypes schema).foldl
    (fun multiplier outputType => max multiplier (listMultiplier listSize outputType)) 1

def maxResponseSizeAlgebra (listSize : Nat) : Algebra :=
  {
    Summary := Nat
    empty := 0
    field := fun group children => 1 + fieldListMultiplier listSize group * children
    combine := Nat.add
    join := max
  }

def caseSizesAlgebra : Algebra :=
  {
    Summary := List Nat
    empty := [0]
    field := fun _group children => children.map (fun size => size + 1)
    combine := fun left right =>
      (left.flatMap fun leftSize => right.map fun rightSize => leftSize + rightSize).mergeSort
    join := fun left right => (left ++ right).mergeSort
  }

def formatBooleanLiteral (literal : BooleanLiteral) : String :=
  s!"{literal.variableName}={if literal.requiredValue then 1 else 0}"

def traceField (group : CollectedFieldGroup) (children : List String) : String :=
  let possibleTypes := group.condition.possibleTypes.mergeSort
  let booleans := group.childInheritedBooleanCondition.map formatBooleanLiteral |>.mergeSort
  let fieldNames := group.fieldNames.mergeSort
  s!"{group.responseName}<{String.intercalate "+" possibleTypes}>" ++
    s!"[{String.intercalate "+" booleans}]" ++
    s!"#{String.intercalate "+" fieldNames}" ++
    "{" ++ String.intercalate "&" children ++ "}"

def sortTraceCases (cases : List (List String)) : List (List String) :=
  cases.mergeSort fun left right =>
    String.intercalate "&" left <= String.intercalate "&" right

def traceAlgebra : Algebra :=
  {
    Summary := List (List String)
    empty := [[]]
    field := fun group children =>
      sortTraceCases <| children.map fun childCase => [traceField group childCase]
    combine := fun left right =>
      sortTraceCases <| left.flatMap fun leftCase =>
        right.map fun rightCase => (leftCase ++ rightCase).mergeSort
    join := fun left right => sortTraceCases (left ++ right)
  }

def analyze (algebra : Algebra) (profile : Profile) : algebra.Summary :=
  let operation := operation profile
  let values := variableValues profile.variableCase
  match profile.mode, profile.variableCase with
  | 0, 0 => ExactCases.summarizeOperation algebra schema operation
  | 0, _ =>
      ExactCases.summarizeOperationWithVariables (fun _values => algebra) schema values operation
  | _, 0 => Syntactic.summarizeOperation algebra schema operation
  | _, _ =>
      Syntactic.summarizeOperationWithVariables (fun _values => algebra) schema values operation

def formatTraceCase (traceCase : List String) : String :=
  if traceCase.isEmpty then "_" else String.intercalate "&" traceCase

def staticCost (profile : Profile) : TreeSummary.StaticCost.Cost :=
  let model : TreeSummary.StaticCost.CostModel :=
    { defaultListSize := profile.listSize }
  let query := operation profile
  let values := variableValues profile.variableCase
  match profile.mode with
  | 0 => TreeSummary.StaticCost.ExactCases.estimateOperationWithVariables
      schema model values query
  | _ => TreeSummary.StaticCost.Syntactic.estimateOperationWithVariables
      schema model values query

def result (profile : Profile) : String :=
  match profile.observation with
  | 0 =>
      let value : Nat := analyze (maxResponseSizeAlgebra profile.listSize) profile
      s!"max:{value}"
  | 1 =>
      let values : List Nat := analyze caseSizesAlgebra profile
      s!"cases:{String.intercalate "," (values.map toString)}"
  | 2 =>
      let cases : List (List String) := analyze traceAlgebra profile
      s!"trace:{String.intercalate "|" (cases.map formatTraceCase)}"
  | _ =>
      let cost := staticCost profile
      s!"cost:{cost.typeCost},{cost.fieldCost}"

def parseBytes (encoded : String) : Option (List Nat) :=
  if encoded == "-" then some []
  else encoded.splitOn "," |>.mapM String.toNat?

def parseRequest (line : String) : Option (String × Profile) := do
  let [version, id, encoded] := line.splitOn " " | none
  if version != "TS2" then none else
    let bytes ← parseBytes encoded
    some (id, profileFromBytes bytes)

partial def serve : IO Unit := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  let line ← stdin.getLine
  if line.isEmpty then
    pure ()
  else
    let line := line.trimAscii.toString
    if line == "quit" then
      pure ()
    else
      match parseRequest line with
      | some (id, profile) => stdout.putStrLn s!"{id}={result profile}"
      | none => stdout.putStrLn "error=invalid-request"
      stdout.flush
      serve

end GraphQLStaticAnalysisFuzz

def main : IO Unit :=
  GraphQLStaticAnalysisFuzz.serve
