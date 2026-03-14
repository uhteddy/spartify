cask "spartify" do
    version "1.0.0"
    sha256 "REPLACE_WITH_SHA256_OF_DMG"

    url "https://github.com/YOUR_USERNAME/spartify/releases/download/v#{version}/Spartify_#{version}_aarch64.dmg"
    name "Spartify"
    desc "Spotify party hosting desktop app"
    homepage "https://github.com/YOUR_USERNAME/spartify"

    app "Spartify.app"
end
