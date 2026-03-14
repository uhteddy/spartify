cask "spartify" do
    version "0.2.1"
    sha256 "REPLACE_WITH_SHA256_OF_DMG"

    url "https://github.com/uhteddy/spartify/releases/download/v#{version}/Spartify_#{version}_aarch64.dmg"
    name "Spartify"
    desc "Spotify party hosting desktop app"
    homepage "https://github.com/uhteddy/spartify"

    app "Spartify.app"
end
