pipeline {
    agent{ label 'docker && ubuntu && !rpi'}

    tools {
        jfrog 'jfrog-cli-latest'
    }

    environment {
        CI="1"
    }

    stages {
        stage ('Setup') {
            steps {
                script {
                    // Retrieve version numbers from canking-gui/package.json and canking-service/Cargo.toml
                    def guiBase = sh(script: 'bash version.sh gui', returnStdout: true).trim()
                    def serviceBase = sh(script: 'bash version.sh service', returnStdout: true).trim()
                    echo "GUI base version (canking-gui/package.json): ${guiBase}"
                    echo "Service base version (canking-service/Cargo.toml): ${serviceBase}"

                    if (env.TAG_NAME) {
                        echo "Build triggered by TAG: ${env.TAG_NAME}"
                        env.GUI_VERSION="${guiBase}"
                        env.SERVICE_VERSION="${serviceBase}"
                    }
                    else if ("${BRANCH_NAME}" == 'main') {
                        // NPM only accepts versions in the format x.y.z or x.y.z-beta.n. We want to keep the version number as x.y.z for main branch,
                        // and use the build number as the beta version for other branches.
                        // The file version is always in the format x.y.z.buildnumber to make it easy to identify the exact build that was used for a specific release candidate.
                        echo "BRANCH_NAME: ${env.BRANCH_NAME}"
                        env.GUI_VERSION="${guiBase}"
                        env.SERVICE_VERSION="${serviceBase}"
                        env.GUI_FILE_VERSION="${guiBase}"
                        env.SERVICE_FILE_VERSION="${serviceBase}"
                    } else {
                        echo "BRANCH_NAME: ${env.BRANCH_NAME}"
                        env.GUI_VERSION="${guiBase}-beta.${env.BUILD_NUMBER}"
                        env.SERVICE_VERSION="${serviceBase}-beta.${env.BUILD_NUMBER}"
                        env.GUI_FILE_VERSION="${guiBase}.${env.BUILD_NUMBER}"
                        env.SERVICE_FILE_VERSION="${serviceBase}.${env.BUILD_NUMBER}"
                    }
                    echo "GUI_VERSION: ${env.GUI_VERSION}, GUI_FILE_VERSION: ${env.GUI_FILE_VERSION}"
                    echo "SERVICE_VERSION: ${env.SERVICE_VERSION}, SERVICE_FILE_VERSION: ${env.SERVICE_FILE_VERSION}"
                }
            }
        }
        stage ('Build/Promote') {
            when { not { buildingTag() } }
            stages {
                stage ('Build') {
                    agent {
                        dockerfile {
                            filename 'Dockerfile'
                            label 'docker && ubuntu && !rpi'
                            reuseNode true
                        }
                    }
                    steps {
                        sh './run.sh rust wasm gui stage'
                        
                        stash includes: 'canking-gui/dist/**,canking-gui/src/a2l-daq-wasm/**', name: 'dist-gui'
                        stash includes: 'canking-service/dist/**', name: 'dist-service'
                    }
                }
                
                stage ('Test') {
                    // Needs node with canlib to run tests
                    agent { label 'vm-ubuntu-24-04' }

                    steps{
                        timeout(time:10, unit: 'MINUTES')
                        {
                            unstash 'dist-gui'
                            unstash 'dist-service'

                            // Install canlib
                            sh'''
                                sudo apt-get update
                                sudo apt-get install -y pkg-config zlib1g-dev libxml2-dev rustup clang
                                rustup toolchain install stable

                                mkdir -p build-linuxcan
                                cd build-linuxcan
                                curl --globoff -Lo linuxcan.tar.gz http://artifact.kvaser.se/artifactory/canlib-generic/linuxcan/[RELEASE]/linuxcan-[RELEASE].tar.gz
                                tar -xzf linuxcan.tar.gz

                                cd linuxcan
                                make
                                sudo make load
                            '''

                            //Install kvlibsdk
                            sh'''
                                mkdir -p build-kvlibsdk
                                cd build-kvlibsdk
                                curl --globoff -Lo kvlibsdk.tar.gz http://artifact.kvaser.se/artifactory/canlib-generic/kvlibsdk/[RELEASE]/kvlibsdk-[RELEASE].tar.gz
                                tar -xzf kvlibsdk.tar.gz

                                cd kvlibsdk
                                make
                                sudo make install

                            '''

                            //Install CanKing Service
                            sh 'echo "deb [trusted=yes] http://artifact.kvaser.se/artifactory/kvaser-debian stable main" | sudo tee /etc/apt/sources.list.d/artifactory.list'

                            sh 'sudo apt update'

                            sh 'sudo apt install -y canking-service'

                            sh 'curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -'
                            sh 'sudo apt install -y nodejs'

                            // Need the transpiled library to build the configs for the tests
                            sh 'cd canking-gui && npm ci'

                            // Run CK cli tests
                            sh 'EXT_WASM="$(readlink -f canking-service/dist/daq_decoder-*.wasm)" CK=/usr/bin/ck REPLAY_MS=3 ./run.sh ck-cli'
                        }
                    }
                }

                stage ('Sign') {
                    // Only sign on main
                    when { branch 'main' }
                    agent { label 'kvaser-sign' }
                    steps{
                        unstash "dist-gui"
                        unstash "dist-service"
                        // Retrieve sign script from artifactory
                        //pwsh "curl --globoff -Lo Sign-CkextBundle.ps1 http://artifact.kvaser.se/artifactory/canking-generic/canking/service/[RELEASE]/CanKingService-[RELEASE].tar.gz!/scripts/Sign-CkextBundle.ps1"
                        pwsh "./Sign-CkextBundle.ps1 -InputBundle canking-gui\\dist -LicenseType Free"
                        pwsh "./Sign-CkextBundle.ps1 -InputBundle canking-service\\dist -LicenseType Free"
                        
                        stash includes: 'canking-gui/dist/**', name: 'dist-gui'
                        stash includes: 'canking-service/dist/**', name: 'dist-service'
                    }
                    post {
                        always {
                            cleanWs()
                        }
                    }
                }

                stage ('Package') {
                    agent {
                        dockerfile {
                            filename 'Dockerfile'
                            label 'docker && ubuntu && !rpi'
                            reuseNode true
                        }
                    }
                    steps {
                        unstash "dist-gui"
                        unstash "dist-service"

                        sh './run.sh pack'

                        stash includes: 'canking-gui/dist/**', name: 'dist-gui'
                        stash includes: 'canking-service/dist/**', name: 'dist-service'
                    }
                }
            }
        }
    }
    post {
        success {
            script{
                if (env.TAG_NAME) {
                    // promote release candidate
                    // promote_artifacts(env.GIT_COMMIT,'canking-generic/canking/extensions/gui/signal-plot/', 'RELEASE-generic', env.TAG_NAME)
                    echo "no promotion at this time, TODO"
                } else if (env.BRANCH_NAME == 'main') {
                    // upload gui ext
                    jf "rt u canking-gui/dist/daq-gui-*.ckext canking-generic/canking/extensions/gui/daq-converter/${env.GUI_FILE_VERSION}/kv_ck7_daq_converter_gui_ext-${env.GUI_FILE_VERSION}.ckext --fail-no-op --target-props version=${env.GUI_FILE_VERSION}"
                    // jf "rt u canking-gui/dist/ReleaseNotes.txt canking-generic/canking/extensions/gui/daq-converter/${env.GUI_FILE_VERSION}/ReleaseNotes-${env.GUI_FILE_VERSION}.txt --fail-no-op --target-props version=${env.GUI_FILE_VERSION}"
                    // upload service ext
                    jf "rt u canking-service/dist/daq_decoder-*.ckext canking-generic/canking/extensions/service/daq-converter/${env.SERVICE_FILE_VERSION}/kv_ck7_daq_converter_service_ext-${env.SERVICE_FILE_VERSION}.ckext --fail-no-op --target-props version=${env.SERVICE_FILE_VERSION}"
                    //jf "rt u dist/ReleaseNotes.txt canking-generic/canking/extensions/gui/daq-converter/${env.SERVICE_FILE_VERSION}/ReleaseNotes-${env.SERVICE_FILE_VERSION}.txt --fail-no-op --target-props version=${env.SERVICE_FILE_VERSION}"
                    jf 'rt bp'
                } else if (env.BRANCH_NAME == 'develop') {
                    // upload gui ext
                    jf "rt u canking-gui/dist/daq-gui-*.ckext canking-generic/extensions/gui/daq-converter/${env.BRANCH_NAME}/${env.GUI_FILE_VERSION}/kv_ck7_daq_converter_gui_ext-${env.GUI_VERSION}.ckext --fail-no-op --target-props version=${env.GUI_VERSION}"
                    //jf "rt u dist/ReleaseNotes.txt canking-generic/extensions/gui/daq-converter/${env.BRANCH_NAME}/${env.GUI_FILE_VERSION}/ReleaseNotes-${env.GUI_VERSION}.txt --fail-no-op --target-props version=${env.GUI_VERSION}"
                    // upload service ext
                    jf "rt u canking-service/dist/daq_decoder-*.ckext canking-generic/extensions/service/daq-converter/${env.BRANCH_NAME}/${env.SERVICE_FILE_VERSION}/kv_ck7_daq_converter_service_ext-${env.SERVICE_VERSION}.ckext --fail-no-op --target-props version=${env.SERVICE_VERSION}"
                    //jf "rt u dist/ReleaseNotes.txt canking-generic/extensions/gui/daq-converter/${env.BRANCH_NAME}/${env.SERVICE_FILE_VERSION}/ReleaseNotes-${env.SERVICE_VERSION}.txt --fail-no-op --target-props version=${env.SERVICE_VERSION}"
                    jf 'rt bp'
                    jf 'rt build-discard --max-builds=5 --delete-artifacts'
                } else {
                    echo "Not uploading artifacts for branch ${env.BRANCH_NAME}"
                }
            }
        }
        changed {
            script {
                if (currentBuild.currentResult == 'FAILURE') {
                    // Send email notifications to the people who made commits in the failing build
                    emailext recipientProviders: [culprits()],
                    subject: "jenkins build:${currentBuild.currentResult}: ${env.JOB_NAME}",
                    body: "${currentBuild.currentResult}: Job ${env.JOB_NAME}\nMore Info can be found here: ${env.BUILD_URL}console",
                    attachLog: true
                }
            }
        }
        cleanup {
			cleanWs()
            sh 'docker image prune -f'
        }
    }
}
