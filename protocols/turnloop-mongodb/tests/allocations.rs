#![deny(unsafe_op_in_unsafe_fn)]
use std::{alloc::{GlobalAlloc,Layout,System},sync::atomic::{AtomicBool,AtomicUsize,Ordering},time::Instant};
use turnloop_mongodb::{bson::{doc,raw::RawDocumentBuf},command::Command,uri::Options,wire::{self,Message},Connection,ConnectionEvent};
struct Counter;
static COUNT:AtomicUsize=AtomicUsize::new(0);
static TRACK:AtomicBool=AtomicBool::new(false);
// SAFETY: Every allocation is forwarded with its original layout to System;
// counting does not access the allocated memory or alter allocation semantics.
unsafe impl GlobalAlloc for Counter{
 unsafe fn alloc(&self,l:Layout)->*mut u8{if TRACK.load(Ordering::Relaxed){COUNT.fetch_add(1,Ordering::Relaxed);} // SAFETY: Forward identical layout to allocator.
 unsafe{System.alloc(l)}}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){ // SAFETY: Pointer and layout come from this allocator's allocation.
 unsafe{System.dealloc(p,l)}}
 unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8{if TRACK.load(Ordering::Relaxed){COUNT.fetch_add(1,Ordering::Relaxed);} // SAFETY: Preserve allocator contract and forward unchanged pointer/layout.
 unsafe{System.realloc(p,l,n)}}
}
#[global_allocator]static ALLOCATOR:Counter=Counter;
#[test]
fn warmed_command_and_borrowed_reply_allocate_zero(){let now=Instant::now();let mut c=Connection::new(Options::parse("mongodb://localhost/").unwrap());c.connected(now,"").unwrap();let n=c.transmit().len();c.consume_transmit(n).unwrap();let hello=RawDocumentBuf::try_from(&doc!{"ok":1,"maxWireVersion":27}).unwrap();let mut reply=Vec::new();wire::encode(&mut reply,2,1,0,&hello,&[],10000).unwrap();feed(&mut c,&reply);assert!(matches!(c.poll_event(),Some(ConnectionEvent::Ready)));
 let filter=RawDocumentBuf::try_from(&doc!{"x":1}).unwrap();let result=RawDocumentBuf::try_from(&doc!{"ok":1,"cursor":{"id":0_i64,"ns":"db.items","firstBatch":[{"x":1},{"x":1}]}}).unwrap();let mut cmd=Command::new();let mut rows=0;
 for round in 0..1002{if round==2{COUNT.store(0,Ordering::SeqCst);TRACK.store(true,Ordering::SeqCst);}cmd.find("db","items",&filter,None).unwrap();c.command(round,cmd.raw(),&[],now).unwrap();let req=Message::parse(c.transmit(),10000).unwrap().request_id;let n=c.transmit().len();c.consume_transmit(n).unwrap();wire::encode(&mut reply,77,req,0,&result,&[],10000).unwrap();feed(&mut c,&reply);assert!(matches!(c.poll_event(),Some(ConnectionEvent::Reply{token})if token==round));let body=c.reply().unwrap();turnloop_mongodb::Error::from_response(body).unwrap();let batch=turnloop_mongodb::command::CursorBatch::parse(body).unwrap();for row in batch.rows(){rows+=row.unwrap().get_i32("x").unwrap();}c.release_reply().unwrap();}
 TRACK.store(false,Ordering::SeqCst);let allocations=COUNT.load(Ordering::SeqCst);assert_eq!(rows,2004);assert_eq!(allocations,0,"1000 warmed commands/2000 rows allocated {allocations} times");}
fn feed(c:&mut Connection,b:&[u8]){let mut at=0;while at<b.len(){let n=c.receive(&b[at..]).unwrap();assert!(n>0);at+=n;}}
