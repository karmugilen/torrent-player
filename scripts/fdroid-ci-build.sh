#!/bin/bash
# Runs inside registry.gitlab.com/fdroid/fdroidserver:buildserver-trixie with
# fdroiddata mounted at /builds/fdroiddata. Mirrors the "fdroid build" job in
# fdroiddata's .gitlab-ci.yml (without publish), so the APK matches F-Droid's.
set -xe
CI_PROJECT_DIR=/builds/fdroiddata
cd $CI_PROJECT_DIR
source /etc/profile.d/bsenv.sh
export ANDROID_HOME=/opt/android-sdk
build=${1:-webtor.app:98}

chown -R $(whoami) .
test -d build || mkdir build
apt-get update
apt-get -y dist-upgrade
sdkmanager "platform-tools" "build-tools;31.0.0"

rm -rf $fdroidserver
git clone --shallow-since=2026-07-13 https://gitlab.com/fdroid/fdroidserver.git $fdroidserver
git -C $fdroidserver config transfer.fsckObjects true
git -C $fdroidserver checkout -B master a35fdfddd9c66823987a410566a6101186e39c84
git -C $fdroidserver pull origin master --ff-only
export PATH="$fdroidserver:$PATH"
export PYTHONPATH="$fdroidserver:$fdroidserver/examples"
export PYTHONUNBUFFERED=true
export serverwebroot=/tmp

git -C $home_vagrant/gradlew-fdroid pull

git -C /tmp/ clone https://gitlab.com/fdroid/fdroid-bootstrap-buildserver.git
export BOOTSTRAP_BUILDSERVER=/tmp/fdroid-bootstrap-buildserver
git -C $BOOTSTRAP_BUILDSERVER config transfer.fsckObjects true
git -C $BOOTSTRAP_BUILDSERVER checkout -B master b33507043e6750065745dd2d8c35d3bb5c633c11
git -C $BOOTSTRAP_BUILDSERVER pull origin master --ff-only
cp $BOOTSTRAP_BUILDSERVER/roles/production_hardening/files/gitconfig $CI_PROJECT_DIR/../.gitconfig

for d in logs tmp unsigned $home_vagrant/.android $home_vagrant/.gradle $home_vagrant/metadata; do
    test -d $d || mkdir $d
    chown -R vagrant $d
done
ln -sfn $home_vagrant/.gradle $CI_PROJECT_DIR/.gradle
ln -sfn $CI_PROJECT_DIR/tmp $home_vagrant/tmp
ln -sfn $CI_PROJECT_DIR/srclibs $home_vagrant/srclibs
sysctl fs.inotify.max_user_watches=524288 || true
export GRADLE_USER_HOME=$home_vagrant/.gradle
fdroid="sudo --preserve-env --user vagrant
      env PATH=$fdroidserver:$PATH
      env PYTHONPATH=$fdroidserver:$fdroidserver/examples
      env PYTHONUNBUFFERED=true
      env TERM=$TERM
      env HOME=$home_vagrant
      fdroid"

apt-get install -y sudo
apt-get install -y openjdk-21-jdk-headless
update-alternatives --set java /usr/lib/jvm/java-21-openjdk-amd64/bin/java
cp -R $CI_PROJECT_DIR/build $home_vagrant/build
appid=${build%:*}
cp -R metadata/$appid.yml $home_vagrant/metadata
chown -R vagrant $home_vagrant $CI_PROJECT_DIR
pushd $home_vagrant
ln -s $CI_PROJECT_DIR $home_vagrant/fdroiddata
ln -s $CI_PROJECT_DIR/../.gitconfig $home_vagrant/.gitconfig
$fdroid fetchsrclibs $build --verbose
rm $home_vagrant/fdroiddata $home_vagrant/.gitconfig
(unset CI; $fdroid build --verbose --test --refresh-scanner --on-server --no-tarball $build) || true
apt-get install -y sudo
popd
# Before a GitHub release exists the Binaries comparison fails after the build;
# scan the APK here as F-Droid does once it matches.
apk=$CI_PROJECT_DIR/tmp/${appid}_${build#*:}.apk
test -f $apk
$fdroid scanner --verbose --exit-code $apk
ls -la $CI_PROJECT_DIR/tmp
