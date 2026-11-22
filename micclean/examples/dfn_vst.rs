use micclean::dsp::Dfn;
use micclean::fx::vst::{VstIo, VstPlugin};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut keep = vec![];
    for p in &a {
        let v = VstPlugin::load(std::path::Path::new(p)).unwrap();
        println!("carregou {} -> dfn ok={}", v.name, Dfn::new(100.0, 0.0).is_ok());
        let mut io = VstIo::new(&v);
        let mut x = vec![0.1f32; 480];
        v.process(&mut io, &mut x);
        println!("processou {} -> dfn ok={}", v.name, Dfn::new(100.0, 0.0).is_ok());
        let st = v.get_state();
        v.set_state(st.0.as_deref(), &st.1);
        println!("estado {} -> dfn ok={}", v.name, Dfn::new(100.0, 0.0).map_err(|e| eprintln!("{e}")).is_ok());
        keep.push(v);
    }
}
