//! Names, slugs or ids → resources. Every command accepts any of them; a name
//! that matches several resources fails listing the candidates.

use crate::api::{Api, enc};
use crate::failure::{Failure, Kind};
use anyhow::Result;
use serde_json::{Value, json};

/// Lowercase, everything that is not a letter or digit becomes `-`.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn label(item: &Value, keys: &[&str]) -> String {
    let name = keys
        .iter()
        .find_map(|k| item[*k].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("?");
    format!("{name} ({})", item["id"].as_str().unwrap_or("?"))
}

/// Picks the item whose id, or one of `keys`, matches `query`.
/// `what` names the resource in Spanish ("proyecto"), `list` is the command that lists them.
pub fn pick(items: &[Value], query: &str, keys: &[&str], what: &str, list: &str) -> Result<Value> {
    let q = query.trim();
    // Ids are uuids: an uppercase copy (urn, a log) is the same id.
    if let Some(item) = items.iter().find(|i| i["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(q))) {
        return Ok(item.clone());
    }
    // Keys in order of preference: a slug that matches wins over a name.
    let mut matches: Vec<&Value> = Vec::new();
    for key in keys {
        matches = items
            .iter()
            .filter(|i| i[*key].as_str().is_some_and(|v| v.eq_ignore_ascii_case(q)))
            .collect();
        if !matches.is_empty() {
            break;
        }
    }
    if matches.is_empty() {
        let qs = slugify(q);
        for key in keys {
            matches = items
                .iter()
                .filter(|i| i[*key].as_str().is_some_and(|v| !qs.is_empty() && slugify(v) == qs))
                .collect();
            if !matches.is_empty() {
                break;
            }
        }
    }
    if matches.is_empty() && q.len() >= 6 {
        matches = items
            .iter()
            .filter(|i| i["id"].as_str().is_some_and(|id| id.starts_with(q)))
            .collect();
    }
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(Failure::new(Kind::NotFound, format!("No hay {what} «{q}».")).hint(list.to_owned()).into()),
        _ => {
            let candidates: Vec<String> = matches.iter().map(|i| label(i, keys)).collect();
            Err(Failure::new(
                Kind::Ambiguous,
                format!("«{q}» coincide con {} {what}s: usa el id.", candidates.len()),
            )
            .details(json!({"candidates": candidates}))
            .hint(list.to_owned())
            .into())
        }
    }
}

pub fn id(item: &Value) -> String {
    item["id"].as_str().unwrap_or_default().to_owned()
}

/// A scoped token sees lists filtered to its projects (`meta.cli_filtered`).
fn note_filtered(envelope: &Value) {
    if envelope["meta"]["cli_filtered"] == true {
        crate::ui::info("La lista está filtrada a los proyectos del token (los totales de la API cuentan también lo que no ves).");
    }
}

/// An organization of the user by id or name. An id that is not in the list
/// goes to the API as is: a scoped token only lists its own organization, and
/// Core is who says why another one is off limits (403 `cli.token_scope`, or
/// not a member) instead of fdt answering "not found".
pub async fn org(api: &Api, query: &str) -> Result<Value> {
    let orgs = orgs(api).await?;
    let list = "fdt org list (un token scoped solo ve su organización: fdt whoami)";
    match pick(&orgs, query, &["name"], "organización", list) {
        Err(_) if uuid::Uuid::parse_str(query.trim()).is_ok() => {
            let id = query.trim().to_lowercase();
            Ok(json!({"id": id, "name": id}))
        }
        other => other,
    }
}

/// The organizations of the user: `GET /users/me/has-organization`.
pub async fn orgs(api: &Api) -> Result<Vec<Value>> {
    let data = api.get("/users/me/has-organization").await?;
    Ok(data["organizations"].as_array().cloned().unwrap_or_default())
}

/// Every project of the org, through all pages.
pub async fn projects(api: &Api, org: &str) -> Result<Vec<Value>> {
    const SIZE: usize = 100;
    let mut all = Vec::new();
    for page in 0..50 {
        let envelope = api
            .envelope(
                reqwest::Method::GET,
                &format!("/organizations/{}/projects", enc(org)),
                &[("page".into(), page.to_string()), ("size".into(), SIZE.to_string())],
                None,
            )
            .await?;
        if page == 0 {
            note_filtered(&envelope);
        }
        let items = envelope["data"].as_array().cloned().unwrap_or_default();
        let n = items.len();
        all.extend(items);
        if n < SIZE {
            break;
        }
    }
    Ok(all)
}

/// The deploy services of the org (`data`), plus `meta` (limits).
pub async fn services_with_meta(api: &Api, org: &str) -> Result<(Vec<Value>, Value)> {
    let mut envelope = api
        .envelope(reqwest::Method::GET, &format!("/organizations/{}/deploy/services", enc(org)), &[], None)
        .await?;
    let items = envelope["data"].as_array().cloned().unwrap_or_default();
    note_filtered(&envelope);
    Ok((items, envelope["meta"].take()))
}

pub async fn services(api: &Api, org: &str) -> Result<Vec<Value>> {
    Ok(services_with_meta(api, org).await?.0)
}

/// The databases of the org (`data.databases`) and the whole list view.
pub async fn databases_view(api: &Api, org: &str) -> Result<Value> {
    api.get(&format!("/organizations/{}/deploy/databases", enc(org))).await
}

pub async fn databases(api: &Api, org: &str) -> Result<Vec<Value>> {
    Ok(databases_view(api, org).await?["databases"].as_array().cloned().unwrap_or_default())
}

pub async fn apps(api: &Api, org: &str) -> Result<Vec<Value>> {
    Ok(api
        .get(&format!("/organizations/{}/apps", enc(org)))
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default())
}

pub async fn project_apps(api: &Api, org: &str, project: &str) -> Result<Vec<Value>> {
    Ok(api
        .get(&format!("/organizations/{}/projects/{}/apps", enc(org), enc(project)))
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default())
}

pub async fn service(api: &Api, org: &str, query: &str) -> Result<Value> {
    pick(&services(api, org).await?, query, &["slug", "name"], "servicio", "fdt service list")
}

pub async fn database(api: &Api, org: &str, query: &str) -> Result<Value> {
    pick(&databases(api, org).await?, query, &["name"], "base de datos", "fdt db list")
}

pub async fn project(api: &Api, org: &str, query: &str) -> Result<Value> {
    pick(&projects(api, org).await?, query, &["name"], "proyecto", "fdt project list")
}

/// An app by id, slug, name or `owner/repo`.
pub fn app(apps: &[Value], query: &str) -> Result<Value> {
    let with_repo: Vec<Value> = apps
        .iter()
        .map(|a| {
            let mut a = a.clone();
            if let Some(full) = a["repository"]["full_name"].as_str().map(str::to_owned) {
                a["repo"] = json!(full);
            }
            a
        })
        .collect();
    pick(&with_repo, query, &["repo", "slug", "name"], "app", "fdt api GET /organizations/{org}/apps")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Value> {
        vec![
            json!({"id": "11111111-aaaa", "name": "Tienda", "slug": "tienda"}),
            json!({"id": "22222222-bbbb", "name": "api", "slug": "api"}),
            json!({"id": "33333333-cccc", "name": "API", "slug": "api-2"}),
        ]
    }

    #[test]
    fn picks_by_id_name_slug_and_prefix() {
        assert_eq!(id(&pick(&items(), "11111111-aaaa", &["slug", "name"], "x", "l").unwrap()), "11111111-aaaa");
        assert_eq!(id(&pick(&items(), "tienda", &["slug", "name"], "x", "l").unwrap()), "11111111-aaaa");
        assert_eq!(id(&pick(&items(), "api-2", &["slug", "name"], "x", "l").unwrap()), "33333333-cccc");
        assert_eq!(id(&pick(&items(), "222222", &["slug", "name"], "x", "l").unwrap()), "22222222-bbbb");
    }

    #[test]
    fn ambiguous_names_list_candidates() {
        let err = pick(&items(), "api", &["name"], "servicio", "fdt service list").unwrap_err();
        let f = crate::failure::failure_of(&err).unwrap();
        assert_eq!(f.kind, Kind::Ambiguous);
        assert_eq!(f.details.as_ref().unwrap()["candidates"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn unknown_names_are_not_found() {
        let err = pick(&items(), "nope", &["name"], "servicio", "fdt service list").unwrap_err();
        assert_eq!(crate::failure::failure_of(&err).unwrap().kind, Kind::NotFound);
        assert_eq!(slugify("Mi Proyecto  2"), "mi-proyecto-2");
    }
}
