/// This is an async WebDAV server.
/// 
/// Design decisions:
/// Right now we allow simple stand-alone filesystem stat calls 
///   "exists()" "is_dir()" "is_file()"
/// to be performed synchronously. All other filesystem and network operations
/// are considered "long-lived" and performed asynchronously.
/// Making these operations in to futures significantly complicates the server
/// so it was decided to leave it until proven necessary. 

// TODO: Error codes are all wrong... like really all wrong
// TODO: PROPFIND, GET, HEAD
// TODO: Locks
// TODO: CarDAV CalDAV


extern crate hyper;
extern crate pretty_env_logger;
extern crate futures;
extern crate tokio;
extern crate tokio_fs;

use hyper::{Body, Request, Response, Server};
use hyper::service::service_fn;
use hyper::rt::{self, Future};
use http::{Method, StatusCode};
use std::path::{Path, PathBuf};
use std::io::Write;
use tokio::fs::file;
use futures::*;

// Maximum alloweable webdav object size
const MAX_FILE_SIZE: u64 = 102400;

type BoxFut = Box<Future<Item=Response<Body>, Error=hyper::http::Error> + Send>;

// ********* Filesystem helper methods

/// Recursively deletes files and directories (not symlinks)
// TODO: Make it not stop on IO Error
fn recursive_delete(path: PathBuf) -> Box<Future<Item=(), Error=std::io::Error> + Send> {
  if path.is_dir() {
    Box::new(tokio::fs::read_dir(path.clone())
      .and_then(|s| s.for_each(|d| 
        recursive_delete(d.path())))
      .and_then(|_| tokio::fs::remove_dir(path))
    )
  } else {
    Box::new(tokio::fs::remove_file(path))
  }
}

/// Recursively copy a src to dest (src=/foo/bar dest=/foo/baz copies bar to baz)
// TODO: Make it not stop on IO Error
fn recursive_copy(src: PathBuf, dest: PathBuf, depth: Option<u32>) -> Box<Future<Item=(), Error=std::io::Error> + Send> {
  fn recursive_copy_helper(src: PathBuf, dest: PathBuf, depth: Option<u32>) -> Box<Future<Item=(), Error=std::io::Error> + Send> {
    if depth == Some(0) {
      return Box::new(done(Ok(())));
    };
    if src.is_dir() {
      return Box::new(tokio::fs::create_dir(dest.clone())
        .and_then(|_| tokio::fs::read_dir(src))
        .and_then(move |s| s.for_each(move |d| 
          recursive_copy(d.path(), 
            Path::join(dest.as_path(), d.path().file_name().unwrap()),
            depth.map(|d| d-1))))
      );
    } else {
      return Box::new(tokio::fs::File::create(dest)
        .and_then(|mut dest_f| tokio::fs::File::open(src)
          .and_then(move |src_f|
            tokio::codec::FramedRead::new(src_f, tokio::codec::BytesCodec::new())
              .for_each(move |c| dest_f.write_all(&c))
          )
        )
      );
    }
  };
  // We want depth=0 to mean delete the top level
  recursive_copy_helper(src, dest, depth.map(|d| d+1))
}

// ********** Path verification helper methods

/// Checks that a path is in the serving path, and not doing dumb things
/// Doesn't check existence, since we may want to create it
fn is_valid_path(p: &Path) -> bool {
	for x in p.components() {
		match x {
			std::path::Component::CurDir => return false,
			std::path::Component::ParentDir => return false, 
			_ => (),
		}
  }
  if !p.starts_with(get_serve_root().as_path()) {
    return false;
  }
  return true;
}

/// Computes a file path from a URI, returns "None" if it's not a valid
fn path_from_uri(uri_path_str: &str) -> Option<PathBuf> {
  // By spec uri might or might not end with "/" when referring to collections
  let uri = uri_path_str.trim_matches('/');
	let path = Path::join(get_serve_root().as_path(), uri);
  // Now do security checks
  if is_valid_path(&path) {Some(path)} else {None}
}

/// Computes file path of parent, returns "None" if it's not a valid 
/// or doesn't exist
fn parent_from_path(path: &Path) -> Option<PathBuf> {
  path.parent().and_then(|p|
    if is_valid_path(p) && p.is_dir() {
      Some(p.to_owned())
    } else {
      None
    }
  )
}

// ********* Server helper methods

fn error_response(statuscode: StatusCode) -> http::Result<Response<Body>> {
  Response::builder().status(statuscode).body(Body::empty())
}

fn get_serve_root() -> PathBuf {
	Path::new("/home/mbrewer/webdav_root").canonicalize().unwrap()
}

/// Get the "depth" header (if it exists).
/// None is infinity, and also the default.
fn get_depth_header<T>(req: &Request<T>) -> Option<u32> {
  req.headers().get("depth").and_then(|v| v.to_str().ok())
    .and_then(|s| if s == "infinity" {None} else {s.parse().ok()})
}

/// Get "destination" header (if it exists).
fn get_dest_header<T>(req: &Request<T>) -> Option<PathBuf> {
  req.headers().get("Destination")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| path_from_uri(s))
}

/// Get "src" header (if it exists).
/// Actually gets it from the URI, just named header for consistancy.
fn get_src_header<T>(req: &Request<T>) -> Option<PathBuf> {
  path_from_uri(req.uri().path())
}

// ************ Main server code

/// Process put requests
fn process_put(req: Request<Body>) -> BoxFut {
  let path = match get_src_header(&req) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  if path.is_dir() {
    return Box::new(done(error_response(StatusCode::METHOD_NOT_ALLOWED)));
  }
  match parent_from_path(&path) {
    None => return Box::new(done(error_response(StatusCode::CONFLICT))), 
    Some(p) => p,
  }; 

  // TODO: We should check for valid XML
  Box::new(file::File::create(path).map_err(|_| StatusCode::NOT_FOUND)
    .and_then(move |mut f| 
      req.into_body().take(MAX_FILE_SIZE).map_err(|_| ()).for_each(move |chunk|
        f.write_all(&chunk).map(|_| ()).map_err(|_| ())
      ).map_err(|_| StatusCode::NOT_FOUND)
    )
    .map(|_| Response::new(Body::from("")))
    .or_else(|status| done(error_response(status)))
    .from_err()
  )
}

/// Process Delete requests
// TODO: must return compound response (Errors encoded in XML)
fn process_delete(req: Request<Body>) -> BoxFut {
	let path =  match get_src_header(&req) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  if !path.exists() {
    return Box::new(done(error_response(StatusCode::NOT_FOUND)));
  }
  Box::new(recursive_delete(path)
    .map(|_| {let mut r = Response::new(Body::from("")); *r.status_mut() = StatusCode::NO_CONTENT; r})
    .or_else(|_| done(error_response(StatusCode::NOT_FOUND)))
    .from_err()
  )
}

/// Process mkcol (webdav extension) requests
fn process_mkcol(req: Request<Body>) -> BoxFut {
  // Gather and verify arguments
  let path = match get_src_header(&req) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };

  // Check
  if !is_valid_path(path.parent().unwrap()) {
    return Box::new(done(error_response(StatusCode::NOT_FOUND))); 
  }; 
  if path.exists() {
    return Box::new(done(error_response(StatusCode::CONFLICT))); 
  };
  // We don't bother checking if the collection file exists
  // This would just be corruption and we'll just truncate it

  // Create
  Box::new(tokio::fs::create_dir(path).map_err(|_| StatusCode::NOT_FOUND)
		.and_then(|_| req.into_body().take(1).collect().map_err(|_| StatusCode::NOT_FOUND)
		// webdav doesn't define what MKCOL body would do, so we error if we get one
		.and_then(|v|
			if v.len() == 0 {
				Ok(())
			} else {
				Err(StatusCode::NOT_FOUND)
			}))
    .map(|_| Response::new(Body::from("")))
    .or_else(|status| done(error_response(status)))
    .from_err()
  )
}

/// Process copy (webdav extension) requests
fn process_copy(req: Request<Body>) -> BoxFut {
  // gather and verify arguments
  let src = match path_from_uri(req.uri().path()) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  let dest = match get_dest_header(&req) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(s) => s, 
  };
  let depth = get_depth_header(&req);

  // check
  if !src.exists() {
    return Box::new(done(error_response(StatusCode::NOT_FOUND)));
  }

  // and copy
  Box::new(recursive_copy(src, dest, depth)
    .map(|_| {let mut r = Response::new(Body::from("")); *r.status_mut() = StatusCode::NO_CONTENT; r})
    .or_else(|_| done(error_response(StatusCode::NOT_FOUND)))
    .from_err()
  )
}

/// Process move (webdav extension) requests
fn process_move(req: Request<Body>) -> BoxFut {
  // Verify paths
  let src = match path_from_uri(req.uri().path()) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  if !src.exists() {
    return Box::new(done(error_response(StatusCode::NOT_FOUND)));
  }
  // Read our headers
  let dest = match get_dest_header(&req) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(s) => s, 
  };
  // TODO: this could probably be "rename"?  
  Box::new(recursive_copy(src.clone(), dest, None).and_then(|_| recursive_delete(src))
    .map(|_| {let mut r = Response::new(Body::from("")); *r.status_mut() = StatusCode::NO_CONTENT; r})
    .or_else(|_| done(error_response(StatusCode::NOT_FOUND)))
    .from_err()
  )
}

/// Process unknown requests
fn process_default(_req: Request<Body>) -> BoxFut {
  Box::new(done(error_response(StatusCode::NOT_FOUND)))
}

fn process_requests(req: Request<Body>) -> BoxFut {
//http::Result<Response<Body>> {
	match *req.method() {
		//Method::OPTIONS => process_default(req),
		//Method::GET => process_default(req),
		//Method::HEAD => process_default(req),
		//Method::POST => process_default(req),
		Method::PUT => process_put(req),
		Method::DELETE => process_delete(req),
		_ => match req.method().as_str() {
			"MKCOL" => process_mkcol(req),
			"MOVE" => process_move(req),
			"COPY" => process_copy(req),
			//"PROPFIND" => process_default(req),
			//"PROPPATCH" => process_default(req),
			_ => process_default(req),
		}
	}
}

fn main() {
  pretty_env_logger::init();
  let addr = ([127, 0, 0, 1], 3000).into();
  let server = Server::bind(&addr)
    .serve(|| service_fn(process_requests))
    .map_err(|e| eprintln!("server error: {}", e));

  println!("Listening on http://{}", addr);

  rt::run(server);
}
