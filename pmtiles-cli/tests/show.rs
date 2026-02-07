use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";

fn pmtiles() -> Command {
    let mut cmd = cargo_bin_cmd!("pmtiles");
    // Fixtures are relative to workspace root
    cmd.current_dir(env!("CARGO_MANIFEST_DIR").to_owned() + "/..");
    cmd
}

#[test]
fn show_raster() {
    pmtiles()
        .args(["show", RASTER_FILE])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("tile type: png")
                .and(predicate::str::contains("min zoom: 0"))
                .and(predicate::str::contains("max zoom: 3"))
                .and(predicate::str::contains("tile compression: none"))
                .and(predicate::str::contains("clustered: true")),
        );
}

#[test]
fn show_vector() {
    pmtiles()
        .args(["show", VECTOR_FILE])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("tile type: mvt")
                .and(predicate::str::contains("tile compression: gzip"))
                .and(predicate::str::contains("clustered: true")),
        );
}

#[test]
fn show_missing_file() {
    pmtiles()
        .args(["show", "nonexistent.pmtiles"])
        .assert()
        .failure();
}

#[test]
fn no_args_shows_help() {
    pmtiles()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}
