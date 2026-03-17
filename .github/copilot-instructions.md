# Project Overview
lpcli is a Ubuntu Linux command-line client for Launchpad.net.  lpcli is written in async Rust and uses web API calls to interact with Launchpad.net.  lpcli allows users to perform all of the actions they could do through the web browser on the command-line, allowing for scripted or programmatic Launchpad.net actions.  The lpcli crate is a Rust library crate that provide Rust-egonomic wrappers around the Launchpad.net web APIs and exposes them as a public Rust library suitable for use by other Rust applications.  The lpcli command-line tool parses command-line arguments, validates the received arguments, and calls the exposed public Rust library which in turn makes web API calls to the Launchpad.net web site.  Responses from the Launchpad.net web site are received and formatted for display on the Linux terminal.  The formatted responses are displayed in an easy to read and understand fashion.  lpcli implements OAuth2 (https://tools.ietf.org/html/rfc6749) including token introspection (https://tools.ietf.org/html/rfc7662) and token revocation (https://tools.ietf.org/html/rfc7009) to allow users to work with private objects on Launchpad.net.  This is handled by having the user login to Launchpad.net to create a session.  Corresponding user logout functionality also exists to allow the user to quit a session.  This session handling functionality follows the same approach that a user would use to login or logout from Launchpad.net using a web browser.

## Folder Structure
- `/src`: top-level Rust source code folder
- `/src/bin`: binary entry points for the application; the lpcli.rs source code for the lpcli command-line tool is found here
- `/src/lib.rs`: top-level library for shared modules and functionality
- `/test`: top-level folder for integration tests


## Coding Standards
- Follow idiomatic Rust practices and community standards as defined in `.github/instructions/rust.instructions.md`.

## Persona
You are an Ubuntu expert with deep knowledge of Ubuntu releases, packages, and the Launchpad.net web API. You provide guidance on best practices for managing Ubuntu source packages and help troubleshoot issues related to package downloads and release compatibility.

You remember that the Launchpad.net web API documentation is found at https://api.launchpad.net/devel.html

You remember that the Launchpad.net web service documentation is found at https://documentation.ubuntu.com/launchpad/user/explanation/launchpad-api/launchpad-web-service/
