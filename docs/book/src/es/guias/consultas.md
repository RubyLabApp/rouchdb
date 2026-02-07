# Consultas

RouchDB ofrece dos formas de consultar documentos: **consultas Mango** (selectores declarativos) y **vistas map/reduce** (funciones personalizadas).

## Consultas Mango

Mango te permite buscar documentos usando selectores JSON, similar a MongoDB:

```rust
use rouchdb::{Database, FindOptions};

let db = Database::memory("mydb");

// Insertar datos de ejemplo
db.put("alice", serde_json::json!({"name": "Alice", "age": 30, "role": "admin"})).await?;
db.put("bob", serde_json::json!({"name": "Bob", "age": 25, "role": "user"})).await?;
db.put("carol", serde_json::json!({"name": "Carol", "age": 35, "role": "admin"})).await?;

// Buscar admins mayores de 28
let result = db.find(FindOptions {
    selector: serde_json::json!({
        "role": "admin",
        "age": {"$gte": 28}
    }),
    ..Default::default()
}).await?;

for doc in &result.docs {
    println!("{}", doc["name"]); // Alice, Carol
}
```

### Operadores disponibles

| Operador | Descripcion | Ejemplo |
|----------|------------|---------|
| `$eq` | Igual a | `{"name": {"$eq": "Alice"}}` o `{"name": "Alice"}` |
| `$ne` | No igual a | `{"role": {"$ne": "admin"}}` |
| `$gt` | Mayor que | `{"age": {"$gt": 25}}` |
| `$gte` | Mayor o igual que | `{"age": {"$gte": 25}}` |
| `$lt` | Menor que | `{"age": {"$lt": 30}}` |
| `$lte` | Menor o igual que | `{"age": {"$lte": 30}}` |
| `$in` | Esta en la lista | `{"role": {"$in": ["admin", "mod"]}}` |
| `$nin` | No esta en la lista | `{"role": {"$nin": ["banned"]}}` |
| `$exists` | El campo existe | `{"email": {"$exists": true}}` |
| `$regex` | Coincide con regex | `{"name": {"$regex": "^A"}}` |
| `$or` | Logico OR | `{"$or": [{"age": 25}, {"age": 30}]}` |
| `$and` | Logico AND | `{"$and": [{"age": {"$gt": 20}}, {"role": "admin"}]}` |
| `$not` | Negacion | `{"age": {"$not": {"$gt": 30}}}` |
| `$elemMatch` | Coincide en array | `{"tags": {"$elemMatch": {"$eq": "rust"}}}` |
| `$all` | Contiene todos | `{"tags": {"$all": ["rust", "db"]}}` |
| `$size` | Tamano del array | `{"tags": {"$size": 3}}` |
| `$mod` | Modulo | `{"age": {"$mod": [5, 0]}}` |
| `$type` | Tipo de valor | `{"name": {"$type": "string"}}` |

### Proyeccion de campos

Selecciona solo los campos que necesitas:

```rust
let result = db.find(FindOptions {
    selector: serde_json::json!({"role": "admin"}),
    fields: Some(vec!["name".into(), "age".into()]),
    ..Default::default()
}).await?;
// Los documentos solo contienen _id, name, age
```

### Ordenamiento

```rust
use rouchdb::SortField;

let result = db.find(FindOptions {
    selector: serde_json::json!({"age": {"$gt": 0}}),
    sort: Some(vec![
        SortField::Simple("age".into()),  // ascendente por defecto
    ]),
    ..Default::default()
}).await?;
```

### Paginacion

```rust
let result = db.find(FindOptions {
    selector: serde_json::json!({"age": {"$gt": 0}}),
    skip: Some(10),
    limit: Some(5),
    ..Default::default()
}).await?;
```

## Vistas Map/Reduce

Para consultas mas complejas, usa `query_view` con funciones map y reduce de Rust:

```rust
use rouchdb::{query_view, ViewQueryOptions, ReduceFn};

// Map: emite pares clave-valor por cada documento
let result = query_view(
    db.adapter(),
    &|doc| {
        // Emitir el rol como clave y 1 como valor
        if let Some(role) = doc.get("role").and_then(|r| r.as_str()) {
            vec![(serde_json::json!(role), serde_json::json!(1))]
        } else {
            vec![]
        }
    },
    Some(&ReduceFn::Count),  // Contar por grupo
    ViewQueryOptions {
        reduce: true,
        group: true,
        ..ViewQueryOptions::new()
    },
).await?;

for row in &result.rows {
    println!("{}: {} usuarios", row.key, row.value);
}
// "admin": 2 usuarios
// "user": 1 usuarios
```

### Reduces integrados

| Funcion | Descripcion |
|---------|-------------|
| `ReduceFn::Sum` | Suma todos los valores numericos |
| `ReduceFn::Count` | Cuenta el numero de filas |
| `ReduceFn::Stats` | Calcula estadisticas: sum, count, min, max, sumsqr |
| `ReduceFn::Custom(fn)` | Funcion reduce personalizada |

### Rangos de claves

```rust
let result = query_view(
    db.adapter(),
    &|doc| {
        let age = doc.get("age").cloned().unwrap_or(serde_json::json!(null));
        let name = doc.get("name").cloned().unwrap_or(serde_json::json!(null));
        vec![(age, name)]
    },
    None,
    ViewQueryOptions {
        start_key: Some(serde_json::json!(25)),
        end_key: Some(serde_json::json!(35)),
        ..ViewQueryOptions::new()
    },
).await?;
```

## Mango vs Map/Reduce: cuando usar cada uno

| Criterio | Mango | Map/Reduce |
|----------|-------|------------|
| **Simplicidad** | Selectores JSON, facil de usar | Requiere escribir closures |
| **Flexibilidad** | Limitado a operadores predefinidos | Logica arbitraria de Rust |
| **Agregaciones** | No soportado | Sum, Count, Stats, Custom |
| **Agrupamiento** | No soportado | group, group_level |
| **Uso tipico** | Filtrar documentos | Reportes y analisis |
