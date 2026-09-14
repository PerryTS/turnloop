//! Hand lowering of asynchronous socket/clock imports, matching wasip3 0.8.0.
use std::net::{SocketAddr,SocketAddrV6,Ipv4Addr,Ipv6Addr};
#[link(wasm_import_module="wasi:clocks/monotonic-clock@0.3.0")]
unsafe extern "C" { #[link_name="[async-lower]wait-until"] pub(super) fn deadline(at:u64)->u32; }
#[link(wasm_import_module="wasi:sockets/types@0.3.0")]
unsafe extern "C" {
    #[link_name="[async-lower][method]tcp-socket.connect"] pub(super) fn connect(params:*mut u32,result:*mut u32)->u32;
    #[link_name="[async-lower][method]udp-socket.send"] pub(super) fn send(params:*mut u32,result:*mut u32)->u32;
    #[link_name="[async-lower][method]udp-socket.receive"] pub(super) fn receive(socket:u32,result:*mut u32)->u32;
}
pub fn encode_addr(storage:&mut [u32],addr:SocketAddr){
    storage.fill(0);
    // SAFETY: exclusive initialized u32 storage, at least 32 bytes, viewed as bytes.
    let bytes=unsafe {std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(),32)};
    bytes[4..6].copy_from_slice(&addr.port().to_le_bytes());
    match addr {
        SocketAddr::V4(a)=>bytes[6..10].copy_from_slice(&a.ip().octets()),
        SocketAddr::V6(a)=>{
            bytes[0]=1;bytes[8..12].copy_from_slice(&a.flowinfo().to_le_bytes());bytes[28..32].copy_from_slice(&a.scope_id().to_le_bytes());
            for (i,s) in a.ip().segments().into_iter().enumerate(){bytes[12+i*2..14+i*2].copy_from_slice(&s.to_le_bytes());}
        }
    }
}
pub fn decode_addr(storage:&[u32])->SocketAddr {
    // SAFETY: caller supplies the initialized 32-byte canonical address.
    let b=unsafe {std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(),32)};
    let port=u16::from_le_bytes([b[4],b[5]]);
    if b[0]==0 {(Ipv4Addr::new(b[6],b[7],b[8],b[9]),port).into()}
    else {let segments=std::array::from_fn(|i|u16::from_le_bytes([b[12+i*2],b[13+i*2]]));SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(segments),port,u32::from_le_bytes(b[8..12].try_into().expect("flow")),u32::from_le_bytes(b[28..32].try_into().expect("scope"))))}
}
