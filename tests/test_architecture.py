"""Regression tests for the dependency guard (standard library only)."""
import unittest

from architecture import inspect_source, tokenize, use_paths


DEPENDENCIES = {name: name for name in (
    "anyhow", "chrono", "serde", "serde_json", "tokio", "sqlx", "axum", "rig", "reqwest",
)}


class ArchitectureGuardTests(unittest.TestCase):
    def inspect(self, source, module=("application", "usecases", "chat"), dependencies=None):
        return inspect_source(source, module, dependencies or DEPENDENCIES)

    def test_nested_grouped_imports(self):
        paths = use_paths(tokenize("crate::{domain::Source, adapters::{spi::{db as store, http}, api::*}}"))
        self.assertEqual(paths, [
            (("crate", "domain", "Source"), "Source"),
            (("crate", "adapters", "spi", "db"), "store"),
            (("crate", "adapters", "spi", "http"), "http"),
            (("crate", "adapters", "api", "*"), "*"),
        ])
        errors = self.inspect("use crate::{domain::Source, adapters::{spi::{db as store, http}}};")
        self.assertEqual(len(errors), 2)
        self.assertTrue(all("outward dependency" in message for _, message in errors))

    def test_domain_cannot_import_application(self):
        self.assertTrue(self.inspect("use crate::application::contracts::ModelMessage;", ("domain", "evidence")))

    def test_relative_paths_respect_inline_modules(self):
        source = "mod tests { use super::super::super::application::contracts::ModelMessage; }"
        self.assertTrue(self.inspect(source, ("domain", "evidence")))
        self.assertFalse(self.inspect("mod tests { use super::*; }", ("domain", "evidence")))

    def test_relative_path_escapes_application(self):
        self.assertTrue(self.inspect("use super::super::super::adapters::spi::db::Store;"))

    def test_direct_qualified_paths(self):
        self.assertTrue(self.inspect("let pool = sqlx::PgPool::connect(url).await;"))
        self.assertTrue(self.inspect("crate::adapters::spi::http::request();"))
        self.assertFalse(self.inspect("crate::domain::evidence::validate(claims);"))

    def test_dependency_and_extern_aliases(self):
        self.assertTrue(self.inspect("use database::PgPool;", dependencies={"database": "sqlx"}))
        self.assertTrue(self.inspect("database::PgPool::connect(url);", dependencies={"database": "sqlx"}))
        self.assertTrue(self.inspect("extern crate reqwest as client; client::get(url);"))

    def test_io_aliases_and_grouped_imports(self):
        for source in (
            "use std::{fs as files, collections::HashMap};",
            "use std as system; system::fs::read(path);",
            "use tokio::{net::{TcpStream}, sync::Mutex};",
            "std::process::Command::new(command);",
            "std::env::var(key);",
            "use std::{self as system}; system::net::TcpStream::connect(addr);",
        ):
            with self.subTest(source=source):
                self.assertTrue(any("I/O belongs" in message for _, message in self.inspect(source)))

    def test_alias_visible_in_inline_child(self):
        self.assertTrue(self.inspect("use std as system; mod child { system::env::var(key); }"))

    def test_io_includes_and_wildcards(self):
        self.assertTrue(self.inspect('let config = include_str!("settings.json");'))
        self.assertTrue(self.inspect('use std::include_str as embed; let config = embed!("settings.json");'))
        self.assertTrue(self.inspect("use std::*;"))
        self.assertTrue(self.inspect('#[path = "../../adapters/spi/model.rs"] mod model;'))

    def test_prompt_text_comments_and_literals_are_ignored(self):
        source = r'''
            // use crate::adapters::spi::db;
            /* use sqlx::PgPool; /* nested comment */ std::env::var("key"); */
            let prompt = r###"use crate::{adapters::spi, application}; "quote""###;
            let bytes = br#"use sqlx::PgPool;"#;
            let c_string = c"reqwest::get(url);";
            let cooked = "quote: \" then sqlx::PgPool";
            let ch = '\'';
        '''
        self.assertFalse(self.inspect(source))

    def test_lifetimes_do_not_hide_paths(self):
        source = "fn read<'a>(value: &'a str) { std::fs::read(value); }"
        self.assertTrue(self.inspect(source))

    def test_api_cannot_call_spi_or_framework_clients(self):
        module = ("adapters", "api", "chat", "controller")
        self.assertTrue(self.inspect("use crate::adapters::spi::db::PgStore;", module))
        self.assertTrue(self.inspect("use rig::completion::Request;", module))
        self.assertFalse(self.inspect("use crate::{application::contracts::StoredMessage, domain::Source}; use axum::Json;", module))

    def test_spi_cross_adapter_coordination_is_allowed(self):
        module = ("adapters", "spi", "db", "embeddings")
        self.assertFalse(self.inspect("use crate::adapters::spi::http::embeddings::Client; use sqlx::PgPool;", module))
        self.assertTrue(self.inspect("use crate::adapters::api::chat::Controller;", module))

    def test_domain_disallows_unreviewed_dependencies(self):
        self.assertTrue(self.inspect("use serde_json::Value;", ("domain", "evidence")))
        self.assertTrue(self.inspect("use novel_crate::Client;", ("domain", "evidence")))

    def test_allowed_computational_dependencies(self):
        source = "use std::{future::Future, pin::Pin, sync::Arc}; use tokio::sync::Mutex; use crate::domain::Source;"
        self.assertFalse(self.inspect(source))

    def test_local_module_reexports(self):
        self.assertFalse(self.inspect("pub mod source_entity; pub use source_entity::{Source, SourceKind};", ("domain",)))

    def test_line_numbers_survive_multiline_literals(self):
        source = 'let prompt = r#"one\ntwo\nthree"#;\nuse sqlx::PgPool;'
        self.assertEqual(self.inspect(source)[0][0], 4)


if __name__ == "__main__":
    unittest.main()
