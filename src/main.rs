extern crate hyper;
extern crate pretty_env_logger;
extern crate futures;
extern crate tokio;
extern crate tokio_fs;

use hyper::{Body, Request, Response, Server};
use hyper::service::service_fn;
use hyper::rt::{self, Future};
use webdav::{Method, StatusCode};
use std::path::{Path, PathBuf};
use std::io::Write;
use tokio::fs::file;
use futures::*;

const MAX_FILE_SIZE: u64 = 102400;

type BoxFut = Box<Future<Item=Response<Body>, Error=hyper::webdav::Error> + Send>;

fn error_response(statuscode: StatusCode) -> webdav::Result<Response<Body>> {
		Response::builder().status(statuscode).body(Body::empty())
}

fn get_serve_root() -> PathBuf {
	Path::new("/home/mbrewer/webdav_root").canonicalize().unwrap()
}

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

/*/// Checks that the parent is in the serving path, valid, and exists
fn has_valid_parent(p: &Path) -> bool {
  let parent = match p.parent() {
    None => return false,
    Some(p) => p,
  };
  is_valid_path(parent) && parent.is_dir()
}
*/

/// Computes a file path from a URI, returns "None" if it's not a valid
fn path_from_uri(uri_path_str: &str) -> Option<PathBuf> {
	let uri_path = Path::new(uri_path_str);	
	uri_path.strip_prefix(Path::new(&"/"))
		.map(|path| Path::join(get_serve_root().as_path(), path))
		.ok()
		// Now do security checks
    .and_then(|p| if is_valid_path(&p) {Some(p)} else {None})
}

/// Computes file path of parent, returns "None" if it's not a valid 
/// or doesn't exist
fn parent_from_path(path: &Path) -> Option<PathBuf> {
  path.parent().and_then(|p|
    // Arguably "is_dir()" should be contained in a future
    if is_valid_path(p) && p.is_dir() {
      Some(p.to_owned())
    } else {
      None
    }
  )
}

/* /// computes a collection metadata file from the path to the collection itself
fn collection_from_path(path: &Path) -> PathBuf {
	// pre:
	//   we checked file is safe (in the serving dir)
  //   we checked the parent is safe (in the serving dir)
	// TODO: Make sure to use a dav-disallowed charactor for ":"
	Path::join(path.parent().unwrap(), String::new() + "C:" + path.file_name().unwrap().to_str().unwrap())
}*/

/// Process put requests
fn process_put(req: Request<Body>) -> BoxFut {
  let path = match path_from_uri(req.uri().path()) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  match parent_from_path(&path) {
    None => return Box::new(done(error_response(StatusCode::CONFLICT))), 
    Some(p) => p,
  }; 

  // At this point "parent" contains all the right errors
  // Next we start with parent, then use path (which if invoked has no errors)
  // This lets us operate on path, while propogating errors properly.

  // Create the file (truncate if exists), and write the stream to it.
  // This is fully async (create, and write_all are both futures).
  //   for_each will wait for new data on the stream, and create a few write_all
  //   future. Write_all will yield () when it completes handing back to for_each.
  // TODO: We should check for valid XML
  Box::new(file::File::create(path).map_err(|_| StatusCode::NOT_FOUND)
    .and_then(move |mut f| 
      req.into_body().take(MAX_FILE_SIZE).for_each(move |chunk| 
        f.write_all(&chunk).map(|_| ()).map_err(hyper::Error::new_io)
      ).map_err(|_| StatusCode::NOT_FOUND)
    )
  // Remap success to a 200 response
    .map(|_| Response::new(Body::from("")))
  // Remap errors to an Ok result containing a response with the status code
  // done() does "Ok(Response)) -> ok(Response)"
    .or_else(|status| done(error_response(status)))
    .from_err()
  )
}

/// Process mkcol (webdav extension) requests
fn process_mkcol(req: Request<Body>) -> BoxFut {
  let path = match path_from_uri(req.uri().path()) {
    None => return Box::new(done(error_response(StatusCode::NOT_FOUND))),
    Some(p) => p,
  };
  // Check the parent exists
  // Unless you run it against "/" (don't do that) parent will exist
  if !is_valid_path(path.parent().unwrap()) {
    return Box::new(done(error_response(StatusCode::CONFLICT))); 
  }; 
  // Check our entity doesn't exist yet 
  if path.exists() {
    return Box::new(done(error_response(StatusCode::CONFLICT))); 
  };
  //let collection = collection_from_path(&path); 
  // We don't bother checking if the collection file exists
  // This would just be corruption and we'll just truncate it
  Box::new(tokio::fs::create_dir(path).map_err(|_| StatusCode::NOT_FOUND)
		.and_then(|_| req.into_body().take(1).collect().map_err(|_| StatusCode::NOT_FOUND)
		// webdav doesn't define what MKCOL body would do, so we error if we get one
		.and_then(|v|
			if v.len() == 0 {
				Ok(())
			} else {
				Err(StatusCode::NOT_FOUND)
			}))
		/*.and_then(move |_|
			file::File::create(collection).map_err(|_| StatusCode::NOT_FOUND))
    .and_then(move |mut f|
      req.into_body().take(MAX_FILE_SIZE).for_each(move |chunk|
        f.write_all(&chunk).map(|_| ()).map_err(hyper::Error::new_io)
      ).map_err(|_| StatusCode::NOT_FOUND)
    )*/
  // Remap success to a 200 response
    .map(|_| Response::new(Body::from("")))
  // Remap errors to an Ok result containing a response with the status code
  // done() does "Ok(Response)) -> ok(Response)"
    .or_else(|status| done(error_response(status)))
    .from_err()
  )

}

fn process_default(_req: Request<Body>) -> BoxFut {
  Box::new(done(error_response(StatusCode::NOT_FOUND)))
}

fn process_requests(req: Request<Body>) -> BoxFut {
//webdav::Result<Response<Body>> {
	match *req.method() {
		//Method::OPTIONS => process_default(req),
		//Method::GET => process_default(req),
		Method::PUT => process_put(req),
		//Method::DELETE => process_default(req),
		_ => match req.method().as_str() {
			"MKCOL" => process_mkcol(req),
			//"PROPFIND" => process_default(req),
			//"MOVE" => process_default(req),
			//"COPY" => process_default(req),
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
