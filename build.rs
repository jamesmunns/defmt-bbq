use std::{
    env,
    path::PathBuf,
    fs::File,
    io::Write,
};

fn main() {
    println!("cargo:rerun-if-env-changed=DEFMT_BBQ_BUFFER_SIZE");

    let size = env::var("DEFMT_BBQ_BUFFER_SIZE")
        .map(|s| {
            s.parse()
                .expect("could not parse DEFMT_BBQ_BUFFER_SIZE as usize")
        })
        .unwrap_or(1024_usize);

    let out_dir_path = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let out_file_path = out_dir_path.join("consts.rs");
    let mut out_file = File::create(&out_file_path).unwrap();

    out_file.write_all(format!("pub(crate) const BUF_SIZE: usize = {};\n", size).as_bytes()).unwrap();

    out_file.flush().unwrap();
}
