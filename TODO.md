🔥 High priority:

[ ✔ ] Create something like manifest file that contains shards ids, their hashes, pieces size
 
[ 🚧 ] Research DHT(distributed hash table)

 1. [ ✔ ] Implement/add DHT.  
 2. [ 🚧 ] Use DHT logic inside sessions.
     - Download logic.
     - Upload logic.
 3. [ ] Own BHT bootstrapping node - postponed. use https://github.com/bittorrent/bootstrap-dht for now. 

🦀 Medium priority:

1. Improve manifest file to better support DHT(see https://en.wikipedia.org/wiki/Torrent_file)
   1. [ ✔ ] Add nodes address in manifest files.
   2. [ ✔ ] Add the target file size in manifest files.
   3. [ ✔ ] Add pieces size in manifest files.
   4. [ ✔ ] Add infohash of a manifest file to uniquely identify a manifest file amount other nodes.

2. Download logic(blocked by DHT research/implementing)   

🐮 Low priority:

1. [ ] Better naming: A "peer" is a client/server listening on a TCP port that implements the BitTorrent protocol. A "node" is a client/server listening on a UDP port implementing the distributed hash table protocol.