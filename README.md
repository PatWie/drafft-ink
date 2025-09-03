# Excalidraw Complete: A Self-Hosted Solution

Excalidraw Complete simplifies the deployment of Excalidraw, bringing an
all-in-one solution to self-hosting this versatile virtual whiteboard. Designed
for ease of setup and use, Excalidraw Complete integrates essential features
into a single Go binary. This solution encompasses:

- The intuitive Excalidraw frontend UI for seamless user experience.
- An integrated data layer ensuring fast and efficient data handling based on different data providers.
- A socket.io implementation to enable real-time collaboration among users.

The project goal is to alleviate the setup complexities traditionally associated with self-hosting Excalidraw, especially in scenarios requiring data persistence and collaborative functionalities.

## Installation

To get started, download the latest release binary:

```bash
# Visit https://github.com/PatWie/excalidraw-complete/releases/ for the download URL
wget <binary-download-url>
chmod +x excalidraw-complete
./excalidraw-complete
```

Once launched, Excalidraw Complete is accessible at `localhost:3002`, ready for
drawing and collaboration.

### Configuration

Excalidraw Complete adapts to your preferences with customizable storage solutions, adjustable via the `STORAGE_TYPE` environment variable:

- **Filesystem:** Opt for `STORAGE_TYPE=filesystem` and define `LOCAL_STORAGE_PATH` to use a local directory.
- **SQLite:** Select `STORAGE_TYPE=sqlite` with `DATA_SOURCE_NAME` for local SQLite storage, including the option for `:memory:` for ephemeral data.
- **AWS S3:** Choose `STORAGE_TYPE=s3` and specify `S3_BUCKET_NAME` to leverage S3 bucket storage, ideal for cloud-based solutions.

These flexible configurations ensure Excalidraw Complete fits seamlessly into your existing setup, whether on-premise or in the cloud.

## Building from Source

Interested in contributing or customizing? Build Excalidraw Complete from source with these steps:

```bash
# Clone and prepare the Excalidraw frontend
git clone https://github.com/PatWie/excalidraw-complete.git --recursive
cd ./excalidraw-complete/excalidraw

# git checkout tags/v0.17.3
# Fix docker build (fix already implemented upstream)
# git remote add jcobol https://github.com/jcobol/excalidraw
# git fetch jcobol
# git checkout 7582_fix_docker_build

# Adjust URLs inside of frontend.patch if you want to use a reverse proxy
git apply ../frontend.patch ## fails recently, you have to change the URLS manually
```
* (Optional)  Remove tracking in excalidraw/excalidraw-app/index.html , ,e.g. change the line above ` <!-- 100% privacy friendly analytics -->`  to ` <% if (typeof PROD == 'undefined') { %>`
```
cd ../
git checkout dev
docker build -t exalidraw-ui-build excalidraw -f ui-build.Dockerfile
docker run -v ${PWD}/:/pwd/ -it exalidraw-ui-build cp -r /frontend /pwd
```

* (Optional) Replace `localhost:3002` inside of `main.go` with your domain name if you want to use a reverse proxy
* (Optional) Replace `"ssl=!0", "ssl=0"` with `"ssl=!0", "ssl=1"` if you want to use HTTPS
* (Optional) Replace `"ssl:!0", "ssl:0"` with `"ssl:!0", "ssl:1"` if you want to use HTTPS
Compile the Go application:

```bash
CGO_ENABLED=0 go build -o excalidraw-complete main.go
```

Declare environment variables if you want any (see section above)
Example: `STORAGE_TYPE=sqlite DATA_SOURCE_NAME=/tmp/excalidb.sqlite`

Start the server:

```bash
go run main.go --listen=":3002"

STORAGE_TYPE=sqlite DATA_SOURCE_NAME=test.db go run main.go --loglevel debug --listen=":3002"
STORAGE_TYPE=filesystem LOCAL_STORAGE_PATH=/tmp/excalidraw/ go run main.go --loglevel debug --listen=":3002"
```

Excalidraw Complete is now running on your machine, ready to bring your collaborative whiteboard ideas to life.

---

Excalidraw is a fantastic tool, but self-hosting it can be tricky. I welcome
your contributions to improve Excalidraw Complete — be it through adding new
features, improving existing ones, or bug reports.
